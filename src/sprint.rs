use anyhow::Result;
use chrono::Local;
use reqwest::Client;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::client::post_json;
use crate::config::{project_jql_clause, AppConfig, Credentials};
use crate::models::SprintStats;

fn parse_datetime(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f%z")
        .ok()
        .or_else(|| chrono::DateTime::parse_from_rfc3339(s).ok())
}

pub fn calc_status_durations(changelog: &Value) -> BTreeMap<String, f64> {
    let mut transitions: Vec<(chrono::DateTime<chrono::FixedOffset>, String, String)> = Vec::new();

    if let Some(histories) = changelog.get("histories").and_then(|h| h.as_array()) {
        for h in histories {
            let ts = h
                .get("created")
                .and_then(|x| x.as_str())
                .and_then(parse_datetime);
            if let Some(ts) = ts {
                if let Some(items) = h.get("items").and_then(|x| x.as_array()) {
                    for item in items {
                        if item.get("field").and_then(|x| x.as_str()) == Some("status") {
                            let from = item
                                .get("fromString")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let to = item
                                .get("toString")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            transitions.push((ts, from, to));
                        }
                    }
                }
            }
        }
    }

    transitions.sort_by_key(|(ts, _, _)| *ts);

    let mut durations: BTreeMap<String, f64> = BTreeMap::new();
    for i in 0..transitions.len() {
        let status = &transitions[i].2;
        let start = transitions[i].0;
        let end = if i + 1 < transitions.len() {
            transitions[i + 1].0
        } else {
            chrono::Local::now().fixed_offset()
        };
        let hours = (end - start).num_minutes() as f64 / 60.0;
        *durations.entry(status.clone()).or_insert(0.0) += hours;
    }

    durations
}

/// Find a sprint entry matching `state` ("active" or "closed") on any issue.
/// For "closed", returns the sprint with the latest endDate (most recently ended).
fn pick_sprint(issues: &[Value], sprint_field: &str, state: &str) -> Option<Value> {
    let mut best: Option<(Value, chrono::DateTime<chrono::FixedOffset>)> = None;
    for issue in issues {
        let sprints = match issue
            .get("fields")
            .and_then(|f| f.get(sprint_field))
            .and_then(|s| s.as_array())
        {
            Some(a) => a,
            None => continue,
        };
        for sp in sprints {
            if sp.get("state").and_then(|x| x.as_str()) != Some(state) {
                continue;
            }
            if state == "active" {
                return Some(sp.clone());
            }
            // For "closed": pick the sprint with the latest endDate.
            let end_dt = sp
                .get("endDate")
                .and_then(|x| x.as_str())
                .and_then(parse_datetime);
            if let Some(dt) = end_dt {
                match &best {
                    Some((_, cur)) if *cur >= dt => {}
                    _ => best = Some((sp.clone(), dt)),
                }
            }
        }
    }
    best.map(|(v, _)| v)
}

pub async fn fetch_sprint_stats(
    client: &Client,
    creds: &Credentials,
    cfg: &AppConfig,
    debug: bool,
) -> Result<Option<SprintStats>> {
    let proj = project_jql_clause(&cfg.jira.projects);
    let proj_and = if proj.is_empty() {
        String::new()
    } else {
        format!(" AND {}", proj)
    };

    let sp_field = &cfg.fields.story_points;
    let sprint_field = &cfg.fields.sprint;

    // Try the active sprint first. If we're between sprints (e.g. morning
    // after a sprint closed), fall back to the most recently closed sprint
    // within the last 2 days so stats don't vanish at the boundary.
    let (issues, sprint_state) = {
        let jql_open = format!(
            "sprint in openSprints() AND assignee = currentUser(){proj_and}",
            proj_and = proj_and,
        );
        if debug {
            eprintln!("[debug] sprint JQL (open): {}", jql_open);
        }
        let body = serde_json::json!({
            "jql": jql_open,
            "fields": [sp_field, sprint_field, "status", "created", "resolutiondate"],
            "expand": "changelog",
            "maxResults": 100
        });
        let v = post_json(client, creds, "/rest/api/3/search/jql", &body).await?;
        let open_issues = v
            .get("issues")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();

        if !open_issues.is_empty() {
            (open_issues, "active")
        } else {
            // Fall back to closed sprints — Jira orders closedSprints() by
            // most recently closed first.
            let jql_closed = format!(
                "sprint in closedSprints() AND assignee = currentUser(){proj_and} \
                 ORDER BY updated DESC",
                proj_and = proj_and,
            );
            if debug {
                eprintln!("[debug] sprint JQL (closed fallback): {}", jql_closed);
            }
            let body = serde_json::json!({
                "jql": jql_closed,
                "fields": [sp_field, sprint_field, "status", "created", "resolutiondate"],
                "expand": "changelog",
                "maxResults": 100
            });
            let v = post_json(client, creds, "/rest/api/3/search/jql", &body).await?;
            let closed_issues = v
                .get("issues")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            (closed_issues, "closed")
        }
    };

    if debug {
        eprintln!(
            "[debug] sprint issues returned: {} (state={})",
            issues.len(),
            sprint_state
        );
    }

    if issues.is_empty() {
        return Ok(None);
    }

    // Pick the sprint matching sprint_state from the issue's sprint array.
    // For "closed", prefer the most recently ended sprint whose endDate is
    // within the last 2 days; otherwise skip the fallback.
    let mut sprint_name = String::new();
    let mut days_remaining: i64 = 0;
    let mut total_days: i64 = 0;
    let mut days_elapsed: i64 = 0;
    let mut state_for_output = sprint_state.to_string();

    let picked = pick_sprint(&issues, sprint_field, sprint_state);
    if let Some(sprint) = picked {
        sprint_name = sprint
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("Unknown Sprint")
            .to_string();
        if let (Some(start), Some(end)) = (
            sprint.get("startDate").and_then(|x| x.as_str()),
            sprint.get("endDate").and_then(|x| x.as_str()),
        ) {
            if let (Some(s), Some(e)) = (parse_datetime(start), parse_datetime(end)) {
                let now = Local::now();
                days_remaining = (e.with_timezone(&Local) - now).num_days();
                total_days = (e - s).num_days();
                days_elapsed = (now - s.with_timezone(&Local)).num_days().max(1);
                // Only surface a closed sprint if it ended in the last 2 days.
                if sprint_state == "closed" && days_remaining < -2 {
                    return Ok(None);
                }
            }
        }
    } else if sprint_state == "closed" {
        // No closed sprint info on the issue → nothing useful to show.
        return Ok(None);
    } else {
        state_for_output = "active".to_string();
    }

    let mut points_done = 0.0_f64;
    let mut points_total = 0.0_f64;
    let mut issues_done = 0usize;
    let issues_total = issues.len();

    let mut resolve_hours: Vec<f64> = Vec::new();
    let mut in_progress_hours: Vec<f64> = Vec::new();
    let mut in_review_hours: Vec<f64> = Vec::new();
    let mut qa_hours: Vec<f64> = Vec::new();
    let mut todo_to_done_hours: Vec<f64> = Vec::new();

    for issue in &issues {
        let fields = issue.get("fields").cloned().unwrap_or(Value::Null);
        let pts = fields
            .get(sp_field.as_str())
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        points_total += pts;

        let is_done = fields
            .get("status")
            .and_then(|s| s.get("statusCategory"))
            .and_then(|c| c.get("key"))
            .and_then(|k| k.as_str())
            == Some("done");

        if is_done {
            points_done += pts;
            issues_done += 1;

            if let (Some(created), Some(resolved)) = (
                fields
                    .get("created")
                    .and_then(|x| x.as_str())
                    .and_then(parse_datetime),
                fields
                    .get("resolutiondate")
                    .and_then(|x| x.as_str())
                    .and_then(parse_datetime),
            ) {
                let h = (resolved - created).num_minutes() as f64 / 60.0;
                if h > 0.0 {
                    resolve_hours.push(h);
                }
            }

            if let Some(changelog) = issue.get("changelog") {
                let durations = calc_status_durations(changelog);
                // Aggregate by configured status categories
                for status_name in &cfg.statuses.in_progress {
                    if let Some(&h) = durations.get(status_name.as_str()) {
                        in_progress_hours.push(h);
                    }
                }
                for status_name in &cfg.statuses.in_review {
                    if let Some(&h) = durations.get(status_name.as_str()) {
                        in_review_hours.push(h);
                    }
                }
                for status_name in &cfg.statuses.qa {
                    if let Some(&h) = durations.get(status_name.as_str()) {
                        qa_hours.push(h);
                    }
                }
                let total: f64 = durations.values().sum();
                if total > 0.0 {
                    todo_to_done_hours.push(total);
                }
            }
        }
    }

    let avg = |v: &[f64]| -> Option<f64> {
        if v.is_empty() {
            None
        } else {
            Some(v.iter().sum::<f64>() / v.len() as f64)
        }
    };

    Ok(Some(SprintStats {
        name: sprint_name,
        state: state_for_output,
        days_remaining,
        total_days,
        days_elapsed,
        points_done,
        points_total,
        issues_done,
        issues_total,
        avg_resolve_hours: avg(&resolve_hours),
        avg_in_progress_hours: avg(&in_progress_hours),
        avg_in_review_hours: avg(&in_review_hours),
        avg_qa_hours: avg(&qa_hours),
        avg_todo_to_done_hours: avg(&todo_to_done_hours),
        points_per_day: if days_elapsed > 0 {
            Some(points_done / days_elapsed as f64)
        } else {
            None
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── parse_datetime ────────────────────────────────────────────────────

    #[test]
    fn parse_datetime_rfc3339() {
        let dt = parse_datetime("2026-04-14T10:30:00+00:00").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-04-14T10:30:00+00:00");
    }

    #[test]
    fn parse_datetime_jira_ms() {
        // Jira's usual shape: milliseconds + offset without colon. Verify by
        // component rather than epoch seconds (avoids hand-computed mistakes).
        let dt = parse_datetime("2026-04-14T10:30:00.123+0000")
            .unwrap()
            .naive_utc();
        assert_eq!(
            dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-04-14 10:30:00"
        );
    }

    #[test]
    fn parse_datetime_rejects_garbage() {
        assert!(parse_datetime("not-a-date").is_none());
        assert!(parse_datetime("").is_none());
    }

    // ── calc_status_durations ─────────────────────────────────────────────

    fn history(created: &str, field: &str, from: &str, to: &str) -> Value {
        json!({
            "created": created,
            "items": [{"field": field, "fromString": from, "toString": to}],
        })
    }

    #[test]
    fn durations_empty_changelog() {
        let cl = json!({ "histories": [] });
        assert!(calc_status_durations(&cl).is_empty());
    }

    #[test]
    fn durations_single_transition_has_open_ended_entry() {
        // Only one transition means the status's end = now(), which is
        // non-deterministic. Just assert the status appears.
        let cl = json!({
            "histories": [history("2026-04-14T10:00:00.000+0000", "status", "To Do", "In Progress")],
        });
        let d = calc_status_durations(&cl);
        assert!(d.contains_key("In Progress"));
    }

    #[test]
    fn durations_two_transitions_deterministic() {
        // 10:00 → In Progress, 12:30 → In Review. "In Progress" = 2.5h.
        // "In Review" is the trailing status; skip asserting on it.
        let cl = json!({
            "histories": [
                history("2026-04-14T10:00:00.000+0000", "status", "To Do", "In Progress"),
                history("2026-04-14T12:30:00.000+0000", "status", "In Progress", "In Review"),
            ],
        });
        let d = calc_status_durations(&cl);
        assert!((d.get("In Progress").copied().unwrap() - 2.5).abs() < 1e-6);
        assert!(d.contains_key("In Review"));
    }

    #[test]
    fn durations_ignores_non_status_items() {
        let cl = json!({
            "histories": [
                history("2026-04-14T10:00:00.000+0000", "assignee", "a", "b"),
                history("2026-04-14T11:00:00.000+0000", "status", "To Do", "Done"),
            ],
        });
        let d = calc_status_durations(&cl);
        // Only the status transition contributes; no "a" or "b" keys.
        assert!(!d.contains_key("a"));
        assert!(!d.contains_key("b"));
        assert!(d.contains_key("Done"));
    }

    #[test]
    fn durations_unordered_histories_sorted_by_time() {
        // Provide histories out of chronological order — should still compute
        // In Progress = 2h (10:00 → 12:00) regardless of input order.
        let cl = json!({
            "histories": [
                history("2026-04-14T12:00:00.000+0000", "status", "In Progress", "Done"),
                history("2026-04-14T10:00:00.000+0000", "status", "To Do", "In Progress"),
            ],
        });
        let d = calc_status_durations(&cl);
        assert!((d.get("In Progress").copied().unwrap() - 2.0).abs() < 1e-6);
    }

    // ── pick_sprint ───────────────────────────────────────────────────────

    fn issue_with_sprints(field: &str, sprints: Value) -> Value {
        json!({ "fields": { field: sprints } })
    }

    fn sprint(name: &str, state: &str, end: Option<&str>) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("name".into(), json!(name));
        o.insert("state".into(), json!(state));
        if let Some(e) = end {
            o.insert("endDate".into(), json!(e));
        }
        Value::Object(o)
    }

    #[test]
    fn pick_sprint_active_returns_first_match() {
        let issues = vec![issue_with_sprints(
            "sp",
            json!([
                sprint("Sprint 41", "closed", Some("2026-04-01T00:00:00+00:00")),
                sprint("Sprint 42", "active", Some("2026-04-15T00:00:00+00:00")),
            ]),
        )];
        let got = pick_sprint(&issues, "sp", "active").unwrap();
        assert_eq!(got.get("name").unwrap().as_str().unwrap(), "Sprint 42");
    }

    #[test]
    fn pick_sprint_closed_picks_latest_end_date() {
        // Two closed sprints across two issues — should pick the one with
        // the later endDate.
        let issues = vec![
            issue_with_sprints(
                "sp",
                json!([sprint(
                    "Sprint 40",
                    "closed",
                    Some("2026-03-20T00:00:00+00:00")
                )]),
            ),
            issue_with_sprints(
                "sp",
                json!([sprint(
                    "Sprint 41",
                    "closed",
                    Some("2026-04-10T00:00:00+00:00")
                )]),
            ),
        ];
        let got = pick_sprint(&issues, "sp", "closed").unwrap();
        assert_eq!(got.get("name").unwrap().as_str().unwrap(), "Sprint 41");
    }

    #[test]
    fn pick_sprint_no_match_returns_none() {
        let issues = vec![issue_with_sprints(
            "sp",
            json!([sprint(
                "Sprint 41",
                "closed",
                Some("2026-04-10T00:00:00+00:00")
            )]),
        )];
        assert!(pick_sprint(&issues, "sp", "active").is_none());
    }

    #[test]
    fn pick_sprint_skips_closed_without_enddate() {
        // Closed sprint without endDate can't be ranked → no pick.
        let issues = vec![issue_with_sprints(
            "sp",
            json!([sprint("Sprint 41", "closed", None)]),
        )];
        assert!(pick_sprint(&issues, "sp", "closed").is_none());
    }

    #[test]
    fn pick_sprint_handles_missing_sprint_field() {
        let issues = vec![json!({ "fields": {} })];
        assert!(pick_sprint(&issues, "sp", "active").is_none());
    }
}
