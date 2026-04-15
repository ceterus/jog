use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;

use serde_json::Value;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Myself {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Activity {
    pub summary: String,
    pub status: String,
    pub transitions: Vec<String>,
    pub my_comments: Vec<String>,
    pub updated_fields: Vec<String>,
    pub assigned_to_me: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct SprintStats {
    pub name: String,
    /// "active" for an open sprint, "closed" when falling back to a recently
    /// closed sprint (e.g. the morning after a sprint closes).
    pub state: String,
    pub days_remaining: i64,
    pub total_days: i64,
    pub days_elapsed: i64,
    pub points_done: f64,
    pub points_total: f64,
    pub issues_done: usize,
    pub issues_total: usize,
    pub avg_resolve_hours: Option<f64>,
    pub avg_in_progress_hours: Option<f64>,
    pub avg_in_review_hours: Option<f64>,
    pub avg_qa_hours: Option<f64>,
    pub avg_todo_to_done_hours: Option<f64>,
    pub points_per_day: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct KanbanStats {
    /// Window used for throughput + cycle-time calculations, e.g. 14.
    pub window_days: i64,
    /// Open work assigned to the user, bucketed by status name.
    pub wip_by_status: BTreeMap<String, usize>,
    /// Total open assigned-to-user work (sum of `wip_by_status`).
    pub wip_total: usize,
    /// Issues completed by the user in the last `window_days`.
    pub throughput: usize,
    /// Average throughput per day across the window.
    pub throughput_per_day: Option<f64>,
    pub avg_resolve_hours: Option<f64>,
    pub avg_in_progress_hours: Option<f64>,
    pub avg_in_review_hours: Option<f64>,
    pub avg_qa_hours: Option<f64>,
    pub avg_todo_to_done_hours: Option<f64>,
}

/// Which flow model applies to this user's work — sprint/scrum or kanban.
/// Serialized with a `type` tag so JSON consumers can branch.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Flow {
    Sprint(SprintStats),
    Kanban(KanbanStats),
}

#[derive(Serialize, Clone, Debug)]
pub struct TodayIssue {
    pub key: String,
    pub summary: String,
    pub status: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct StandupData {
    pub user_name: String,
    /// Start of the "since last standup" window (YYYY-MM-DD).
    pub start_date: String,
    /// End of the window — usually now (YYYY-MM-DD HH:MM).
    pub end_datetime: String,
    /// Human label for the section header, e.g. "Since Friday".
    pub since_label: String,
    pub activities: BTreeMap<String, Activity>,
    pub today: Vec<TodayIssue>,
    pub flow: Option<Flow>,
}

pub fn derive_me_from_issues(issues: &[Value]) -> Option<Myself> {
    if let (Ok(id), Ok(name)) = (env::var("JIRA_ACCOUNT_ID"), env::var("JIRA_DISPLAY_NAME")) {
        return Some(Myself {
            account_id: id,
            display_name: name,
        });
    }
    if let Ok(id) = env::var("JIRA_ACCOUNT_ID") {
        return Some(Myself {
            account_id: id,
            display_name: "you".to_string(),
        });
    }
    for issue in issues {
        if let Some(a) = issue.get("fields").and_then(|f| f.get("assignee")) {
            let id = a.get("accountId").and_then(|x| x.as_str()).unwrap_or("");
            let name = a
                .get("displayName")
                .and_then(|x| x.as_str())
                .unwrap_or("you");
            if !id.is_empty() {
                return Some(Myself {
                    account_id: id.to_string(),
                    display_name: name.to_string(),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn derive_me_picks_first_assignee() {
        // Env-var path only triggers if JIRA_ACCOUNT_ID is set; don't set it
        // here and rely on the issue-array fallback.
        let issues = vec![
            json!({"fields": {"assignee": null}}),
            json!({"fields": {"assignee": {"accountId": "abc123", "displayName": "Jane"}}}),
        ];
        let me = derive_me_from_issues(&issues).unwrap();
        assert_eq!(me.account_id, "abc123");
        assert_eq!(me.display_name, "Jane");
    }

    #[test]
    fn derive_me_defaults_display_name_when_missing() {
        let issues = vec![json!({"fields": {"assignee": {"accountId": "x"}}})];
        let me = derive_me_from_issues(&issues).unwrap();
        assert_eq!(me.account_id, "x");
        assert_eq!(me.display_name, "you");
    }

    #[test]
    fn derive_me_returns_none_when_no_assignees() {
        // Guard: skip if the user running tests has JIRA_ACCOUNT_ID set —
        // derive_me would happily pick that up and succeed.
        if env::var("JIRA_ACCOUNT_ID").is_ok() {
            return;
        }
        let issues = vec![json!({"fields": {}}), json!({"fields": {"assignee": null}})];
        assert!(derive_me_from_issues(&issues).is_none());
    }

    #[test]
    fn derive_me_skips_assignees_with_empty_account_id() {
        let issues = vec![
            json!({"fields": {"assignee": {"accountId": "", "displayName": "Nobody"}}}),
            json!({"fields": {"assignee": {"accountId": "real", "displayName": "Someone"}}}),
        ];
        let me = derive_me_from_issues(&issues).unwrap();
        assert_eq!(me.account_id, "real");
    }
}
