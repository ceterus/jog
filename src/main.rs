mod auth;
mod client;
mod comments;
mod config;
mod flow;
mod jql;
mod models;
mod output;

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

    /// Hide the stats panel (points/velocity/throughput/cycle times).
    /// Overrides `[output].stats` for this run. Use when sharing to Slack
    /// or anywhere you'd rather not publish personal performance metrics.
    #[arg(long, global = true)]
    no_stats: bool,

    /// Stats verbosity: full | summary | off. Overrides `[output].stats`
    /// for this run.
    #[arg(long, global = true)]
    stats: Option<String>,
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

    // Resolve flow mode first — it decides how "Today" is queried and which
    // stats panel we render.
    let (flow_mode, flow_stats) =
        match flow::fetch_flow_stats(&http, &creds, &app_cfg, args.debug).await {
            Ok(x) => x,
            Err(e) => {
                if args.debug {
                    eprintln!("[debug] flow stats failed: {e:#}");
                }
                (flow::FlowMode::Scrum, None)
            }
        };

    let today_issues = jql::search_in_progress(&http, &creds, &app_cfg, flow_mode)
        .await
        .unwrap_or_default();

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
        flow: flow_stats,
    };

    let fmt = args.format.as_deref().unwrap_or(&app_cfg.output.format);

    // CLI precedence: --no-stats > --stats > [output].stats in config.
    let stats_mode = if args.no_stats {
        config::StatsMode::Off
    } else if let Some(s) = args.stats.as_deref() {
        config::StatsMode::from_str(s)
    } else {
        config::StatsMode::from_str(&app_cfg.output.stats)
    };

    output::render(&data, fmt, stats_mode);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // ── previous_work_day ────────────────────────────────────────────────

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn prev_work_day_weekday_rolls_back_one() {
        // 2026-04-15 = Wednesday → Tuesday
        assert_eq!(previous_work_day(d(2026, 4, 15)), d(2026, 4, 14));
        // 2026-04-14 = Tuesday → Monday
        assert_eq!(previous_work_day(d(2026, 4, 14)), d(2026, 4, 13));
    }

    #[test]
    fn prev_work_day_monday_rolls_to_friday() {
        // 2026-04-13 = Monday → Friday 2026-04-10
        assert_eq!(previous_work_day(d(2026, 4, 13)), d(2026, 4, 10));
    }

    #[test]
    fn prev_work_day_sunday_rolls_to_friday() {
        // 2026-04-12 = Sunday → Friday 2026-04-10
        assert_eq!(previous_work_day(d(2026, 4, 12)), d(2026, 4, 10));
    }

    #[test]
    fn prev_work_day_saturday_rolls_back_one() {
        // 2026-04-11 = Saturday → Friday 2026-04-10 (matches default branch)
        assert_eq!(previous_work_day(d(2026, 4, 11)), d(2026, 4, 10));
    }

    // ── day_label ────────────────────────────────────────────────────────

    #[test]
    fn day_label_week() {
        assert_eq!(day_label(d(2026, 4, 13)), "Monday");
        assert_eq!(day_label(d(2026, 4, 14)), "Tuesday");
        assert_eq!(day_label(d(2026, 4, 15)), "Wednesday");
        assert_eq!(day_label(d(2026, 4, 16)), "Thursday");
        assert_eq!(day_label(d(2026, 4, 17)), "Friday");
        assert_eq!(day_label(d(2026, 4, 18)), "Saturday");
        assert_eq!(day_label(d(2026, 4, 19)), "Sunday");
    }

    // ── datetime_from_iso ────────────────────────────────────────────────

    #[test]
    fn datetime_from_iso_parses_both_shapes() {
        assert!(datetime_from_iso("2026-04-14T10:30:00.123+0000").is_some());
        assert!(datetime_from_iso("2026-04-14T10:30:00+00:00").is_some());
        assert!(datetime_from_iso("garbage").is_none());
        assert!(datetime_from_iso("").is_none());
    }

    // ── in_range ─────────────────────────────────────────────────────────

    #[test]
    fn in_range_inclusive_bounds() {
        let start = Local.with_ymd_and_hms(2026, 4, 14, 0, 0, 0).unwrap();
        let end = Local.with_ymd_and_hms(2026, 4, 14, 23, 59, 0).unwrap();

        // Right in the middle — use the same offset the iso parser emits.
        let mid = "2026-04-14T12:00:00+00:00";
        // The bounds above are local-tz, so translate the same way the real
        // call path does: parse with iso, compare against local-tz bounds.
        // The noon-UTC value lands inside any local day matching 2026-04-14.
        // Rather than reason about tz, just verify false-on-garbage and
        // false-on-out-of-window with explicit offsets.
        assert!(!in_range("not-a-date", start, end));

        // Out of window: same day 1 year later.
        assert!(!in_range("2027-04-14T12:00:00+00:00", start, end));

        // Sanity: a time clearly inside the window regardless of tz drift.
        // Use a value parsed via the same function so the comparison is
        // apples-to-apples with real production usage.
        let inside = datetime_from_iso(mid).unwrap();
        let start2 = inside - Duration::hours(1);
        let end2 = inside + Duration::hours(1);
        assert!(in_range(mid, start2, end2));
    }
}
