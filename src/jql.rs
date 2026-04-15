use anyhow::Result;
use chrono::{DateTime, Local};
use reqwest::Client;
use serde_json::Value;

use crate::client::post_json;
use crate::config::{AppConfig, Credentials, project_jql_clause, done_statuses_jql};
use crate::models::TodayIssue;

pub async fn search_updated(
    client: &Client,
    creds: &Credentials,
    cfg: &AppConfig,
    start: DateTime<Local>,
    end: DateTime<Local>,
) -> Result<Vec<Value>> {
    let proj = project_jql_clause(&cfg.jira.projects);
    let proj_clause = if proj.is_empty() {
        String::new()
    } else {
        format!("{} AND ", proj)
    };

    let s = start.format("%Y-%m-%d %H:%M").to_string();
    let e = end.format("%Y-%m-%d %H:%M").to_string();

    // No sprint filter here: activity is time-scoped, not sprint-scoped.
    // At sprint boundaries, the previous day's work may sit in a just-closed
    // sprint; filtering by openSprints() would drop it.
    let jql = format!(
        "{proj_clause}\
         updated >= \"{s}\" AND updated <= \"{e}\" AND (assignee = currentUser() OR \
         status CHANGED BY currentUser() DURING (\"{s}\", \"{e}\") OR \
         worklogAuthor = currentUser() OR reporter = currentUser())",
        proj_clause = proj_clause,
        s = s,
        e = e,
    );

    let fields = vec![
        "summary", "status", "issuetype", "priority", "project", "assignee",
    ];

    let mut out = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = serde_json::json!({
            "jql": jql,
            "fields": fields,
            "expand": "changelog",
            "maxResults": 50
        });
        if let Some(t) = &next_token {
            body["nextPageToken"] = Value::String(t.clone());
        }
        let v = post_json(client, creds, "/rest/api/3/search/jql", &body).await?;
        let issues = v
            .get("issues")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        out.extend(issues);
        let is_last = v.get("isLast").and_then(|x| x.as_bool()).unwrap_or(true);
        next_token = v
            .get("nextPageToken")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        if is_last || next_token.is_none() {
            break;
        }
    }
    Ok(out)
}

pub async fn search_in_progress(
    client: &Client,
    creds: &Credentials,
    cfg: &AppConfig,
) -> Result<Vec<TodayIssue>> {
    let proj = project_jql_clause(&cfg.jira.projects);
    let proj_clause = if proj.is_empty() {
        String::new()
    } else {
        format!("{} AND ", proj)
    };
    let done = done_statuses_jql(&cfg.statuses.done);

    let jql = format!(
        "assignee = currentUser() AND sprint in openSprints() AND {proj_clause}\
         status NOT IN ({done}) ORDER BY status ASC, updated DESC",
        proj_clause = proj_clause,
        done = done,
    );
    let body = serde_json::json!({
        "jql": jql,
        "fields": ["summary", "status"],
        "maxResults": 15
    });
    let v = post_json(client, creds, "/rest/api/3/search/jql", &body).await?;
    let issues = v
        .get("issues")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(issues
        .iter()
        .filter_map(|i| {
            let key = i.get("key")?.as_str()?.to_string();
            let f = i.get("fields")?;
            let summary = f.get("summary")?.as_str()?.to_string();
            let status = f.get("status")?.get("name")?.as_str()?.to_string();
            Some(TodayIssue { key, summary, status })
        })
        .collect())
}
