mod auth;
mod client;
mod comments;
mod config;
mod jql;
mod models;
mod output;
mod sprint;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Weekday};
use clap::{Parser, Subcommand};
use reqwest::Client;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::comments::{adf_to_text, comments_on};
use crate::config::Credentials;
use crate::models::{Activity, Myself, StandupData};

#[derive(Parser, Debug)]
#[command(about = "Jira standup summary for previous work day")]
struct Args {
    #[command(subcommand)]
    command: Option<Sub>,

    /// Override target date (YYYY-MM-DD). Default: previous work day.
    #[arg(long, global = true)]
    date: Option<String>,

    /// Output format: text, json, markdown
    #[arg(long, global = true)]
    format: Option<String>,

    /// Print raw debug info
    #[arg(long, global = true)]
    debug: bool,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Store Jira credentials in macOS Keychain
    Setup,
    /// Show current config (masked token)
    Config,
}

fn previous_work_day(today: NaiveDate) -> NaiveDate {
    match today.weekday() {
        Weekday::Mon => today - Duration::days(3),
        Weekday::Sun => today - Duration::days(2),
        _ => today - Duration::days(1),
    }
}

fn day_label(date: NaiveDate) -> &'static str {
    match date.weekday() {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

fn datetime_from_iso(s: &str) -> Option<DateTime<Local>> {
    chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f%z")
        .ok()
        .map(|dt| dt.with_timezone(&Local))
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Local))
        })
}

fn in_range(s: &str, start: DateTime<Local>, end: DateTime<Local>) -> bool {
    match datetime_from_iso(s) {
        Some(dt) => dt >= start && dt <= end,
        None => false,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match &args.command {
        Some(Sub::Setup) => return auth::run_setup(),
        Some(Sub::Config) => {
            auth::run_config();
            return Ok(());
        }
        None => {}
    }

    let app_cfg = config::load_config();
    let http = Client::builder().build()?;
    let creds = Credentials::resolve(&app_cfg)?;

    let start_date = if let Some(s) = &args.date {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").context("bad --date")?
    } else {
        previous_work_day(Local::now().date_naive())
    };

    // Window = [start_date 00:00 local, now]. Captures prev work day + today so far.
    let start_dt: DateTime<Local> = Local
        .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .ok_or_else(|| anyhow!("bad start datetime"))?;
    let end_dt: DateTime<Local> = Local::now();

    let issues = jql::search_updated(&http, &creds, &app_cfg, start_dt, end_dt).await?;

    let me = match client::get_json(&http, &creds, "/rest/api/3/myself").await {
        Ok(v) => serde_json::from_value::<Myself>(v)?,
        Err(e) => {
            if args.debug {
                eprintln!("[debug] /myself failed: {e}. Deriving accountId from issues.");
            }
            models::derive_me_from_issues(&issues)
                .ok_or_else(|| anyhow!("/myself failed and no issues to derive accountId. Set JIRA_ACCOUNT_ID env var or run `jog setup`."))?
        }
    };

    if args.debug {
        eprintln!("[debug] user={} ({})", me.display_name, me.account_id);
        eprintln!(
            "[debug] window={} → {} issues_returned={}",
            start_dt.format("%Y-%m-%d %H:%M"),
            end_dt.format("%Y-%m-%d %H:%M"),
            issues.len()
        );
        eprintln!("[debug] config={}", config::config_path().display());
    }

    // Build activities from issues
    let mut activities: BTreeMap<String, Activity> = BTreeMap::new();

    for issue in &issues {
        let key = issue
            .get("key")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if key.is_empty() {
            continue;
        }
        let fields = issue.get("fields").cloned().unwrap_or(Value::Null);
        let summary = fields
            .get("summary")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let status = fields
            .get("status")
            .and_then(|x| x.get("name"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let assignee_id = fields
            .get("assignee")
            .and_then(|x| x.get("accountId"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let mut act = Activity {
            summary,
            status,
            transitions: vec![],
            my_comments: vec![],
            updated_fields: vec![],
            assigned_to_me: !assignee_id.is_empty() && assignee_id == me.account_id,
        };

        if let Some(histories) = issue
            .get("changelog")
            .and_then(|x| x.get("histories"))
            .and_then(|x| x.as_array())
        {
            for h in histories {
                let author_id = h
                    .get("author")
                    .and_then(|x| x.get("accountId"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if author_id != me.account_id {
                    continue;
                }
                let created = h.get("created").and_then(|x| x.as_str()).unwrap_or("");
                if !in_range(created, start_dt, end_dt) {
                    continue;
                }
                if let Some(items) = h.get("items").and_then(|x| x.as_array()) {
                    for it in items {
                        let field = it.get("field").and_then(|x| x.as_str()).unwrap_or("");
                        let from = it.get("fromString").and_then(|x| x.as_str()).unwrap_or("");
                        let to = it.get("toString").and_then(|x| x.as_str()).unwrap_or("");
                        if field == "status" {
                            act.transitions.push(format!("{} → {}", from, to));
                        } else if !field.is_empty() {
                            act.updated_fields.push(field.to_string());
                        }
                    }
                }
            }
        }

        if let Ok(issue_comments) = comments_on(&http, &creds, &key).await {
            for c in issue_comments {
                let author_id = c
                    .get("author")
                    .and_then(|x| x.get("accountId"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if author_id != me.account_id {
                    continue;
                }
                let created = c.get("created").and_then(|x| x.as_str()).unwrap_or("");
                if !in_range(created, start_dt, end_dt) {
                    continue;
                }
                let body = c.get("body").map(adf_to_text).unwrap_or_default();
                if !body.is_empty() {
                    act.my_comments.push(body);
                }
            }
        }

        let has_signal = !act.transitions.is_empty()
            || !act.my_comments.is_empty()
            || !act.updated_fields.is_empty();
        if has_signal {
            activities.insert(key, act);
        }
    }

    let today_issues = jql::search_in_progress(&http, &creds, &app_cfg)
        .await
        .unwrap_or_default();
    let sprint_stats = match sprint::fetch_sprint_stats(&http, &creds, &app_cfg, args.debug).await {
        Ok(s) => s,
        Err(e) => {
            if args.debug {
                eprintln!("[debug] sprint stats failed: {e:#}");
            }
            None
        }
    };

    // Label: "Since Friday" when prev workday != yesterday,
    // "Since yesterday" when it is.
    let today = Local::now().date_naive();
    let since_label = if today - start_date == Duration::days(1) {
        "Since yesterday".to_string()
    } else {
        format!("Since {}", day_label(start_date))
    };

    let data = StandupData {
        user_name: me.display_name,
        start_date: start_date.format("%Y-%m-%d").to_string(),
        end_datetime: end_dt.format("%Y-%m-%d %H:%M").to_string(),
        since_label,
        activities,
        today: today_issues,
        sprint: sprint_stats,
    };

    let fmt = args
        .format
        .as_deref()
        .unwrap_or(&app_cfg.output.format);

    output::render(&data, fmt);
    Ok(())
}
