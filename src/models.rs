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
    /// Non-status field edits made during the window. Captured as
    /// per-change rows (field + from + to) so renderers can show the
    /// actual values, collapse repeated edits, and alias field names.
    pub updated_fields: Vec<FieldChange>,
    pub assigned_to_me: bool,
}

/// One non-status field change pulled from a Jira changelog item.
/// `from` / `to` are the `fromString` / `toString` values (empty string
/// when Jira reports the change as a clear/set).
#[derive(Serialize, Clone, Debug)]
pub struct FieldChange {
    pub field: String,
    pub from: String,
    pub to: String,
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
    /// Issues resolved per day across the sprint, oldest-first, length =
    /// `days_elapsed` (or `1` if the sprint just started). Powers the
    /// sparkline row in the stats card.
    pub done_per_day: Vec<u32>,
    /// Burndown reconstructed from issue changelogs. `None` if the sprint
    /// has no start/end date or no issues have a usable changelog.
    pub burndown: Option<Burndown>,
}

/// Reconstructed per-day remaining-points series for an active sprint,
/// plus any mid-sprint scope changes. Built by replaying issue changelogs
/// (Sprint field, story points field, status transitions).
#[derive(Serialize, Clone, Debug)]
pub struct Burndown {
    /// Remaining points at the end of each elapsed day, oldest-first.
    /// Index 0 = scope at sprint start (before day 1). Length =
    /// `days_elapsed + 1`.
    pub series: Vec<f64>,
    /// Linear projection of remaining points for each future day at the
    /// current observed velocity. Oldest-first, length = days-left.
    /// Empty when the sprint is over or velocity is unknown.
    pub projection: Vec<f64>,
    /// Mid-sprint scope events in chronological order.
    pub scope_changes: Vec<ScopeChange>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopeChangeKind {
    Added,
    Removed,
    Repointed,
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct ScopeChange {
    /// 1-indexed day of sprint when the change landed (D1, D2, ...).
    pub day: usize,
    /// Signed points delta: + for added scope, - for removed.
    pub delta_pts: f64,
    pub kind: ScopeChangeKind,
}

/// Per-day trend extras for Kanban flow. Powers throughput + WIP
/// sparklines in the stats card. All series are oldest-first and aligned
/// to `window_days`.
#[derive(Serialize, Clone, Debug, Default)]
pub struct KanbanTrend {
    /// Issues open (assigned to user, not done) at end of each day.
    /// Length = `window_days`.
    pub wip_per_day: Vec<u32>,
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
    /// Issues resolved per day across `window_days`, oldest-first.
    /// Powers the sparkline row in the stats card.
    pub done_per_day: Vec<u32>,
    /// Per-day trend series (currently: WIP count). `None` when the
    /// changelog data needed to reconstruct the series was unavailable.
    pub trend: Option<KanbanTrend>,
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

/// Derived review state for an OPEN PR. Completed PRs leave this as `None`.
/// Precedence (highest wins): Draft > ChangesRequested > NeedsReply >
/// ReadyToMerge > NeedsReview.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrStatus {
    Draft,
    ChangesRequested,
    NeedsReply,
    ReadyToMerge,
    NeedsReview,
}

impl PrStatus {
    pub fn label(&self) -> &'static str {
        match self {
            PrStatus::Draft => "DRAFT",
            PrStatus::ChangesRequested => "CHANGES",
            PrStatus::NeedsReply => "REPLY",
            PrStatus::ReadyToMerge => "READY",
            PrStatus::NeedsReview => "REVIEW",
        }
    }
}

/// A single Bitbucket pull request, normalised into what we need for
/// standup output.
#[derive(Serialize, Clone, Debug)]
pub struct PullRequest {
    pub id: u64,
    pub title: String,
    /// "workspace/repo-slug"
    pub repo: String,
    /// "OPEN" | "MERGED" | "DECLINED" | "SUPERSEDED"
    pub state: String,
    pub url: String,
    pub created_on: String,
    pub updated_on: String,
    pub approvals: u64,
    /// Count of participants with role=REVIEWER (includes those who haven't
    /// approved yet). Not every workspace uses explicit reviewers, so this
    /// can legitimately be 0.
    pub reviewers: u64,
    /// Top-level comment threads with no reply, excluding the PR author's
    /// own top-level comments and outdated inline comments.
    pub unreplied_comments: u64,
    /// True if any participant has `state=changes_requested`.
    pub changes_requested: bool,
    /// True if the PR is in Bitbucket's draft state.
    pub is_draft: bool,
    /// Derived review state for OPEN PRs; `None` for MERGED/DECLINED.
    pub status: Option<PrStatus>,
}

impl PullRequest {
    /// Derive status from the already-populated review fields. Only
    /// meaningful for OPEN PRs.
    pub fn derive_status(&self) -> Option<PrStatus> {
        if self.state != "OPEN" {
            return None;
        }
        if self.is_draft {
            return Some(PrStatus::Draft);
        }
        if self.changes_requested {
            return Some(PrStatus::ChangesRequested);
        }
        if self.unreplied_comments > 0 {
            return Some(PrStatus::NeedsReply);
        }
        if self.approvals > 0 {
            return Some(PrStatus::ReadyToMerge);
        }
        Some(PrStatus::NeedsReview)
    }
}

/// Bitbucket-shaped standup section. All three lists are pre-classified
/// so output rendering can stay dumb.
#[derive(Serialize, Clone, Debug, Default)]
pub struct BitbucketActivity {
    /// PRs authored by me and created within the standup window.
    pub opened: Vec<PullRequest>,
    /// PRs authored by me that reached a terminal state (merged/declined)
    /// within the standup window.
    pub completed: Vec<PullRequest>,
    /// Older open PRs authored by me that still need approval (created
    /// before the window started, no approvals yet).
    pub awaiting_approval: Vec<PullRequest>,
}

impl BitbucketActivity {
    pub fn is_empty(&self) -> bool {
        self.opened.is_empty() && self.completed.is_empty() && self.awaiting_approval.is_empty()
    }
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
    pub bitbucket: Option<BitbucketActivity>,
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
