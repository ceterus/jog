use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use crate::models::{BitbucketActivity, PullRequest};

/// Bitbucket Cloud accepts the same Atlassian API token as Jira via Basic
/// auth (`email:token`). `token` may be either the main jog token (if it's
/// scoped broadly enough) or a separate Bitbucket-scoped API token.
///
/// `projects` optionally restricts the repo fan-out to a subset of Bitbucket
/// projects (empty = scan every repo in the workspace, which can be slow on
/// large orgs).
pub struct BitbucketCredentials {
    pub workspace: String,
    pub email: String,
    pub token: String,
    pub projects: Vec<String>,
}

impl BitbucketCredentials {
    fn auth_header(&self) -> String {
        use base64::{engine::general_purpose, Engine as _};
        let raw = format!("{}:{}", self.email, self.token);
        format!("Basic {}", general_purpose::STANDARD.encode(raw))
    }
}

const API_BASE: &str = "https://api.bitbucket.org/2.0";
const MAX_PAGES: u32 = 10; // cap at 500 PRs (50 per page) — plenty for a standup
/// Cap total repo fan-out per run so a large workspace doesn't hang the
/// standup. If a user genuinely contributes to more repos than this, we
/// silently truncate with a debug note.
const MAX_REPOS: usize = 100;
/// Max in-flight concurrent per-repo PR requests. Balances "go fast" with
/// "don't hammer Bitbucket into rate-limiting the user".
const CONCURRENCY: usize = 10;

/// Fetch the current user's Bitbucket UUID via /user. Bitbucket uses UUIDs,
/// not the Jira accountId, so we can't reuse our Jira identity.
async fn fetch_my_uuid(client: &Client, creds: &BitbucketCredentials) -> Result<String> {
    let url = format!("{API_BASE}/user");
    let v: Value = client
        .get(&url)
        .header("Authorization", creds.auth_header())
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET /user")?
        .error_for_status()
        .context("GET /user status")?
        .json()
        .await
        .context("GET /user body")?;
    let uuid = v
        .get("uuid")
        .and_then(|x| x.as_str())
        .context("no uuid in /user response")?
        .to_string();
    Ok(uuid)
}

/// Fetch PR activity for the window `[start, now]` and classify into the
/// three standup buckets.
///
/// Atlassian removed the workspace-wide `/pullrequests/{selected_user}`
/// endpoint during the GDPR/username cleanup, so we iterate repos in the
/// workspace and query PRs per-repo with an `author.uuid` filter.
pub async fn fetch_activity(
    client: &Client,
    creds: &BitbucketCredentials,
    start: DateTime<Local>,
    debug: bool,
) -> Result<BitbucketActivity> {
    let uuid = fetch_my_uuid(client, creds).await?;
    if debug {
        eprintln!("[debug] bitbucket uuid: {}", uuid);
    }

    let repos = fetch_workspace_repos(client, creds, debug).await?;
    if debug {
        eprintln!(
            "[debug] bitbucket: scanning {} repos in workspace {}",
            repos.len(),
            creds.workspace,
        );
    }

    // Inline progress: show a single-line spinner+counter that streams as
    // each repo's PR query completes. Hidden under `--debug` (debug println!s
    // would interleave and corrupt the carriage-return redraws) and hidden
    // when stderr isn't a TTY (piped / JSON consumers / CI).
    let pb = progress_bar(repos.len() as u64, debug);

    // Bounded-concurrency fan-out: cap in-flight requests at CONCURRENCY
    // so we don't open 100 sockets at once and trip Bitbucket rate limits.
    let results: Vec<(String, Result<Vec<PullRequest>>)> = stream::iter(repos.iter().cloned())
        .map(|repo_slug| {
            let uuid = uuid.clone();
            // ProgressBar is internally Arc-backed; cloning is cheap.
            let pb = pb.clone();
            async move {
                let r = fetch_repo_prs(client, creds, &repo_slug, &uuid, start, debug).await;
                if let Some(pb) = pb {
                    pb.inc(1);
                }
                (repo_slug, r)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    let mut all: Vec<PullRequest> = Vec::new();
    let mut repos_with_hits = 0;
    for (repo_slug, res) in results {
        match res {
            Ok(mut prs) => {
                if !prs.is_empty() {
                    if debug {
                        eprintln!("[debug] bitbucket: repo {} → {} PRs", repo_slug, prs.len());
                    }
                    repos_with_hits += 1;
                }
                all.append(&mut prs);
            }
            Err(e) => {
                if debug {
                    eprintln!("[debug] bitbucket: repo {} failed: {e:#}", repo_slug);
                }
                // Skip this repo but keep going — a single 403 on a repo we
                // can't see shouldn't kill the whole summary.
            }
        }
    }
    if debug {
        eprintln!(
            "[debug] bitbucket: {} PRs across {} repos",
            all.len(),
            repos_with_hits
        );
    }

    Ok(classify(&all, start))
}

/// Build a stderr progress bar for the BB repo fan-out, or `None` when
/// we shouldn't render one (debug mode corrupts output; non-TTY is a
/// hint we're piped into a file/another process).
fn progress_bar(total: u64, debug: bool) -> Option<ProgressBar> {
    if debug {
        return None;
    }
    // Respect piping: indicatif auto-detects TTY, but we also want the
    // bar on stderr specifically so `jog --format json > file.json`
    // stays clean.
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return None;
    }
    let pb = ProgressBar::new(total).with_finish(indicatif::ProgressFinish::AndClear);
    pb.set_draw_target(indicatif::ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} Bitbucket: scanning repos [{bar:20.cyan/blue}] {pos}/{len}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    Some(pb)
}

/// Page through `/repositories/{workspace}` and collect repo slugs,
/// optionally filtered by Bitbucket project keys.
async fn fetch_workspace_repos(
    client: &Client,
    creds: &BitbucketCredentials,
    debug: bool,
) -> Result<Vec<String>> {
    let q_param = if creds.projects.is_empty() {
        String::new()
    } else {
        // BBQL OR chain. Keys are typically short caps like "CRM" but we
        // quote them anyway to survive unusual characters.
        let clauses: Vec<String> = creds
            .projects
            .iter()
            .map(|p| format!("project.key=\"{}\"", p.trim()))
            .collect();
        let q = clauses.join(" OR ");
        format!("&q={}", urlencoding::encode(&q))
    };
    let mut next_url = Some(format!(
        "{API_BASE}/repositories/{ws}?pagelen=100&fields=values.slug,next{q_param}",
        ws = urlencoding::encode(&creds.workspace),
    ));
    if debug && !creds.projects.is_empty() {
        eprintln!(
            "[debug] bitbucket: filtering repos to projects {:?}",
            creds.projects
        );
    }
    let mut slugs: Vec<String> = Vec::new();
    let mut pages = 0u32;

    while let Some(u) = next_url {
        pages += 1;
        if pages > MAX_PAGES {
            if debug {
                eprintln!("[debug] bitbucket: hit MAX_PAGES on repo listing, truncating");
            }
            break;
        }
        let v: Value = client
            .get(&u)
            .header("Authorization", creds.auth_header())
            .header("Accept", "application/json")
            .send()
            .await
            .context("bitbucket repositories GET")?
            .error_for_status()
            .context("bitbucket repositories status")?
            .json()
            .await
            .context("bitbucket repositories body")?;

        if let Some(values) = v.get("values").and_then(|x| x.as_array()) {
            for r in values {
                if let Some(slug) = r.get("slug").and_then(|x| x.as_str()) {
                    slugs.push(slug.to_string());
                    if slugs.len() >= MAX_REPOS {
                        if debug {
                            eprintln!(
                                "[debug] bitbucket: hit MAX_REPOS ({}) cap, truncating",
                                MAX_REPOS
                            );
                        }
                        return Ok(slugs);
                    }
                }
            }
        }
        next_url = v
            .get("next")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
    }
    Ok(slugs)
}

/// Fetch PRs in one repo authored by the current user that are either OPEN
/// or transitioned to a terminal state inside the standup window.
async fn fetch_repo_prs(
    client: &Client,
    creds: &BitbucketCredentials,
    repo_slug: &str,
    uuid: &str,
    start: DateTime<Local>,
    debug: bool,
) -> Result<Vec<PullRequest>> {
    // Flat BBQL: author + date window. Any OR-nested expression in BBQL
    // silently matches nothing in practice, so we use a single-conjunction
    // form. Caveat: stale OPEN PRs (no activity in the window) won't
    // appear in the awaiting-approval bucket — a known limitation.
    let start_iso = start.to_rfc3339();
    let q = format!("author.uuid=\"{uuid}\" AND updated_on >= {start_iso:?}");
    let base = format!(
        "{API_BASE}/repositories/{ws}/{repo}/pullrequests?pagelen=50&sort=-updated_on&q={q}&state=OPEN&state=MERGED&state=DECLINED",
        ws = urlencoding::encode(&creds.workspace),
        repo = urlencoding::encode(repo_slug),
        q = urlencoding::encode(&q),
    );
    if debug && repo_slug == "playmakercrm" {
        eprintln!("[debug] bitbucket: sample URL for {}: {}", repo_slug, base);
    }

    let mut next_url = Some(base);
    let mut out: Vec<PullRequest> = Vec::new();
    let mut pages = 0u32;

    while let Some(u) = next_url {
        pages += 1;
        if pages > MAX_PAGES {
            if debug {
                eprintln!(
                    "[debug] bitbucket: hit MAX_PAGES on repo {} PRs, truncating",
                    repo_slug
                );
            }
            break;
        }
        let v: Value = client
            .get(&u)
            .header("Authorization", creds.auth_header())
            .header("Accept", "application/json")
            .send()
            .await
            .context("bitbucket pullrequests GET")?
            .error_for_status()
            .context("bitbucket pullrequests status")?
            .json()
            .await
            .context("bitbucket pullrequests body")?;

        if let Some(values) = v.get("values").and_then(|x| x.as_array()) {
            for pr in values {
                if let Some(parsed) = parse_pr(pr, &creds.workspace) {
                    out.push(parsed);
                }
            }
        }
        next_url = v
            .get("next")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
    }
    Ok(out)
}

/// Parse one PR JSON object into our model, filtering to the target workspace.
fn parse_pr(pr: &Value, workspace: &str) -> Option<PullRequest> {
    // Destination repo full_name is "workspace/repo-slug".
    let repo = pr
        .get("destination")
        .and_then(|d| d.get("repository"))
        .and_then(|r| r.get("full_name"))
        .and_then(|n| n.as_str())?
        .to_string();
    if !repo.starts_with(&format!("{workspace}/")) {
        return None;
    }

    let id = pr.get("id").and_then(|x| x.as_u64())?;
    let title = pr
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("(no title)")
        .to_string();
    let state = pr
        .get("state")
        .and_then(|x| x.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();
    let url = pr
        .get("links")
        .and_then(|l| l.get("html"))
        .and_then(|h| h.get("href"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let created_on = pr
        .get("created_on")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let updated_on = pr
        .get("updated_on")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let approvals = pr
        .get("participants")
        .and_then(|x| x.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("approved").and_then(|x| x.as_bool()).unwrap_or(false))
                .count() as u64
        })
        .unwrap_or(0);

    Some(PullRequest {
        id,
        title,
        repo,
        state,
        url,
        created_on,
        updated_on,
        approvals,
    })
}

/// Split a raw PR list into the three standup buckets. See
/// [`BitbucketActivity`] docs for bucket semantics.
///
/// Bucket rules (given only PRs already date-filtered by the BBQL query):
/// - `completed`: MERGED or DECLINED (updated within window implicit).
/// - `opened`: OPEN and created within the window.
/// - `awaiting_approval`: OPEN, NOT created in window, 0 approvals —
///   i.e. an older PR that was nudged this window (comment, push, etc.)
///   but still hasn't cleared review.
fn classify(prs: &[PullRequest], window_start: DateTime<Local>) -> BitbucketActivity {
    let mut opened = Vec::new();
    let mut completed = Vec::new();
    let mut awaiting_approval = Vec::new();

    for pr in prs {
        let created = parse_bb_datetime(&pr.created_on);
        let in_window_created = created.map(|d| d >= window_start).unwrap_or(false);

        if pr.state == "MERGED" || pr.state == "DECLINED" {
            completed.push(pr.clone());
            continue;
        }
        if pr.state == "OPEN" {
            if in_window_created {
                opened.push(pr.clone());
            } else if pr.approvals == 0 {
                awaiting_approval.push(pr.clone());
            }
        }
        // Other states (SUPERSEDED etc.) are intentionally dropped.
    }

    BitbucketActivity {
        opened,
        completed,
        awaiting_approval,
    }
}

fn parse_bb_datetime(s: &str) -> Option<DateTime<Local>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Local))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn pr(state: &str, created_on: &str, approvals: u64) -> PullRequest {
        PullRequest {
            id: 1,
            title: "t".into(),
            repo: "org/repo".into(),
            state: state.into(),
            url: "".into(),
            created_on: created_on.into(),
            updated_on: created_on.into(),
            approvals,
        }
    }

    fn window_start() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 4, 14, 0, 0, 0).unwrap()
    }

    #[test]
    fn classify_merged_goes_to_completed() {
        let prs = vec![pr("MERGED", "2026-04-14T12:00:00+00:00", 2)];
        let a = classify(&prs, window_start());
        assert_eq!(a.completed.len(), 1);
        assert!(a.opened.is_empty());
        assert!(a.awaiting_approval.is_empty());
    }

    #[test]
    fn classify_declined_goes_to_completed() {
        let prs = vec![pr("DECLINED", "2026-04-14T12:00:00+00:00", 0)];
        let a = classify(&prs, window_start());
        assert_eq!(a.completed.len(), 1);
    }

    #[test]
    fn classify_open_in_window_goes_to_opened() {
        let prs = vec![pr("OPEN", "2026-04-14T12:00:00+00:00", 0)];
        let a = classify(&prs, window_start());
        assert_eq!(a.opened.len(), 1);
        assert!(a.awaiting_approval.is_empty());
    }

    #[test]
    fn classify_open_outside_window_with_zero_approvals_goes_to_awaiting() {
        let prs = vec![pr("OPEN", "2026-04-10T12:00:00+00:00", 0)];
        let a = classify(&prs, window_start());
        assert!(a.opened.is_empty());
        assert_eq!(a.awaiting_approval.len(), 1);
    }

    #[test]
    fn classify_open_outside_window_but_approved_stays_hidden() {
        // Already has an approval — don't nag the user about it.
        let prs = vec![pr("OPEN", "2026-04-10T12:00:00+00:00", 1)];
        let a = classify(&prs, window_start());
        assert!(a.opened.is_empty());
        assert!(a.awaiting_approval.is_empty());
        assert!(a.completed.is_empty());
    }

    #[test]
    fn classify_superseded_dropped() {
        let prs = vec![pr("SUPERSEDED", "2026-04-14T12:00:00+00:00", 0)];
        let a = classify(&prs, window_start());
        assert!(a.is_empty());
    }

    #[test]
    fn parse_pr_rejects_wrong_workspace() {
        let body = json!({
            "id": 1,
            "title": "t",
            "state": "OPEN",
            "created_on": "2026-04-14T12:00:00+00:00",
            "updated_on": "2026-04-14T12:00:00+00:00",
            "destination": {"repository": {"full_name": "other-org/repo"}},
            "links": {"html": {"href": "https://bitbucket.org/other-org/repo/pull-requests/1"}},
            "participants": [],
        });
        assert!(parse_pr(&body, "myorg").is_none());
    }

    #[test]
    fn parse_pr_counts_approvals() {
        let body = json!({
            "id": 42,
            "title": "Add retry",
            "state": "OPEN",
            "created_on": "2026-04-14T12:00:00+00:00",
            "updated_on": "2026-04-14T12:00:00+00:00",
            "destination": {"repository": {"full_name": "myorg/api"}},
            "links": {"html": {"href": "https://bitbucket.org/myorg/api/pull-requests/42"}},
            "participants": [
                {"approved": true},
                {"approved": false},
                {"approved": true},
            ],
        });
        let p = parse_pr(&body, "myorg").unwrap();
        assert_eq!(p.id, 42);
        assert_eq!(p.repo, "myorg/api");
        assert_eq!(p.approvals, 2);
    }

    #[test]
    fn activity_is_empty_when_no_prs() {
        assert!(BitbucketActivity::default().is_empty());
    }
}
