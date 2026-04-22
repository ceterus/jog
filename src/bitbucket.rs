use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use crate::models::{BitbucketActivity, PullRequest};

/// Aggregated review info extracted from a PR's `participants[]` array.
#[derive(Default, Debug, Clone, Copy)]
struct ReviewInfo {
    approvals: u64,
    reviewers: u64,
    changes_requested: bool,
}

/// Aggregated comment-thread info for one PR.
#[derive(Default, Debug, Clone, Copy)]
struct CommentInfo {
    unreplied: u64,
}

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
                let r = fetch_repo_prs_with_reviews(client, creds, &repo_slug, &uuid, start, debug)
                    .await;
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

/// Wrap `fetch_repo_prs` and enrich each OPEN PR with comment-thread info.
/// Completed PRs (MERGED/DECLINED) skip the extra call — their status
/// badge is not rendered.
async fn fetch_repo_prs_with_reviews(
    client: &Client,
    creds: &BitbucketCredentials,
    repo_slug: &str,
    uuid: &str,
    start: DateTime<Local>,
    debug: bool,
) -> Result<Vec<PullRequest>> {
    let mut prs = fetch_repo_prs(client, creds, repo_slug, uuid, start, debug).await?;
    // OPEN PRs need two enrichment calls:
    //   1. PR detail — the list endpoint's `participants[]` is often stale
    //      or missing the `approved`/`state` flags, so re-fetch per PR.
    //   2. Comments — for the unreplied-thread tally.
    // Completed PRs (MERGED/DECLINED) skip both — they show no status badge.
    let open_indices: Vec<usize> = prs
        .iter()
        .enumerate()
        .filter(|(_, pr)| pr.state == "OPEN")
        .map(|(i, _)| i)
        .collect();
    let enrichments = stream::iter(open_indices.iter().cloned())
        .map(|i| {
            let pr_id = prs[i].id;
            async move {
                let review = fetch_pr_review_info(client, creds, repo_slug, pr_id, debug)
                    .await
                    .unwrap_or_default();
                let comments = fetch_pr_comments(client, creds, repo_slug, pr_id, uuid, debug)
                    .await
                    .unwrap_or_default();
                (i, review, comments)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    for (i, review, comments) in enrichments {
        prs[i].approvals = review.approvals;
        prs[i].reviewers = review.reviewers;
        prs[i].changes_requested = review.changes_requested;
        prs[i].unreplied_comments = comments.unreplied;
        prs[i].status = prs[i].derive_status();
    }
    Ok(prs)
}

/// Fetch a single PR's detail and re-extract review info. Works around
/// the list endpoint's stale/incomplete `participants[]` payload.
async fn fetch_pr_review_info(
    client: &Client,
    creds: &BitbucketCredentials,
    repo_slug: &str,
    pr_id: u64,
    debug: bool,
) -> Result<ReviewInfo> {
    let url = format!(
        "{API_BASE}/repositories/{ws}/{repo}/pullrequests/{pr_id}",
        ws = urlencoding::encode(&creds.workspace),
        repo = urlencoding::encode(repo_slug),
    );
    let v: Value = client
        .get(&url)
        .header("Authorization", creds.auth_header())
        .header("Accept", "application/json")
        .send()
        .await
        .context("bitbucket pr detail GET")?
        .error_for_status()
        .context("bitbucket pr detail status")?
        .json()
        .await
        .context("bitbucket pr detail body")?;
    let info = extract_review_info(&v);
    if debug {
        eprintln!(
            "[debug] bitbucket: {} PR {} detail → {}/{} approved, changes_requested={}",
            repo_slug, pr_id, info.approvals, info.reviewers, info.changes_requested
        );
    }
    Ok(info)
}

/// Fetch and tally comment threads on one PR. Counts top-level threads
/// that: (a) weren't posted by the PR author (our current user), (b) have
/// no reply of any kind, (c) aren't deleted, and (d) for inline comments,
/// aren't marked outdated.
async fn fetch_pr_comments(
    client: &Client,
    creds: &BitbucketCredentials,
    repo_slug: &str,
    pr_id: u64,
    author_uuid: &str,
    debug: bool,
) -> Result<CommentInfo> {
    let base = format!(
        "{API_BASE}/repositories/{ws}/{repo}/pullrequests/{pr_id}/comments?pagelen=100",
        ws = urlencoding::encode(&creds.workspace),
        repo = urlencoding::encode(repo_slug),
    );
    let mut next_url = Some(base);
    let mut all: Vec<Value> = Vec::new();
    let mut pages = 0u32;
    while let Some(u) = next_url {
        pages += 1;
        if pages > MAX_PAGES {
            if debug {
                eprintln!(
                    "[debug] bitbucket: hit MAX_PAGES on {} PR {} comments",
                    repo_slug, pr_id
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
            .context("bitbucket pr comments GET")?
            .error_for_status()
            .context("bitbucket pr comments status")?
            .json()
            .await
            .context("bitbucket pr comments body")?;
        if let Some(values) = v.get("values").and_then(|x| x.as_array()) {
            all.extend(values.iter().cloned());
        }
        next_url = v
            .get("next")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
    }
    Ok(tally_unreplied(&all, author_uuid))
}

/// Pure tally over a flat comment list. Factored out so we can unit-test
/// the thread/author/outdated logic without a live API.
fn tally_unreplied(comments: &[Value], author_uuid: &str) -> CommentInfo {
    use std::collections::HashSet;
    // Set of parent IDs that have at least one reply.
    let mut parents_with_replies: HashSet<u64> = HashSet::new();
    for c in comments {
        if let Some(parent_id) = c
            .get("parent")
            .and_then(|p| p.get("id"))
            .and_then(|x| x.as_u64())
        {
            parents_with_replies.insert(parent_id);
        }
    }
    let mut unreplied = 0u64;
    for c in comments {
        // Skip replies — we only count top-level threads.
        if c.get("parent").and_then(|p| p.get("id")).is_some() {
            continue;
        }
        if c.get("deleted").and_then(|x| x.as_bool()).unwrap_or(false) {
            continue;
        }
        // Author's own top-level comments don't count.
        let user_uuid = c
            .get("user")
            .and_then(|u| u.get("uuid"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if user_uuid == author_uuid {
            continue;
        }
        // Inline comment marked outdated — author fixed the code referenced.
        let outdated = c
            .get("inline")
            .and_then(|i| i.get("outdated"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if outdated {
            continue;
        }
        let id = match c.get("id").and_then(|x| x.as_u64()) {
            Some(i) => i,
            None => continue,
        };
        if parents_with_replies.contains(&id) {
            continue;
        }
        unreplied += 1;
    }
    CommentInfo { unreplied }
}

/// Inspect `participants[]` for approvals, reviewer role count, and any
/// `state=changes_requested` flag.
fn extract_review_info(pr: &Value) -> ReviewInfo {
    let parts = match pr.get("participants").and_then(|x| x.as_array()) {
        Some(p) => p,
        None => return ReviewInfo::default(),
    };
    let mut info = ReviewInfo::default();
    for p in parts {
        if p.get("approved").and_then(|x| x.as_bool()).unwrap_or(false) {
            info.approvals += 1;
        }
        if p.get("role").and_then(|x| x.as_str()) == Some("REVIEWER") {
            info.reviewers += 1;
        }
        if p.get("state").and_then(|x| x.as_str()) == Some("changes_requested") {
            info.changes_requested = true;
        }
    }
    info
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
    let review = extract_review_info(pr);
    let is_draft = pr.get("draft").and_then(|x| x.as_bool()).unwrap_or(false);

    let mut out = PullRequest {
        id,
        title,
        repo,
        state,
        url,
        created_on,
        updated_on,
        approvals: review.approvals,
        reviewers: review.reviewers,
        unreplied_comments: 0,
        changes_requested: review.changes_requested,
        is_draft,
        status: None,
    };
    // Set initial status from what we know here; it's refined after the
    // comments fetch populates `unreplied_comments`.
    out.status = out.derive_status();
    Some(out)
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
        let mut p = PullRequest {
            id: 1,
            title: "t".into(),
            repo: "org/repo".into(),
            state: state.into(),
            url: "".into(),
            created_on: created_on.into(),
            updated_on: created_on.into(),
            approvals,
            reviewers: 0,
            unreplied_comments: 0,
            changes_requested: false,
            is_draft: false,
            status: None,
        };
        p.status = p.derive_status();
        p
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

    #[test]
    fn parse_pr_extracts_reviewers_and_changes_requested() {
        let body = json!({
            "id": 7,
            "title": "t",
            "state": "OPEN",
            "created_on": "2026-04-14T12:00:00+00:00",
            "updated_on": "2026-04-14T12:00:00+00:00",
            "draft": false,
            "destination": {"repository": {"full_name": "myorg/api"}},
            "links": {"html": {"href": "https://bitbucket.org/myorg/api/pull-requests/7"}},
            "participants": [
                {"role": "REVIEWER", "approved": true, "state": "approved"},
                {"role": "REVIEWER", "approved": false, "state": "changes_requested"},
                {"role": "REVIEWER", "approved": false, "state": null},
                {"role": "PARTICIPANT", "approved": false, "state": null},
            ],
        });
        let p = parse_pr(&body, "myorg").unwrap();
        assert_eq!(p.approvals, 1);
        assert_eq!(p.reviewers, 3);
        assert!(p.changes_requested);
        assert_eq!(p.status, Some(crate::models::PrStatus::ChangesRequested));
    }

    #[test]
    fn parse_pr_detects_draft() {
        let body = json!({
            "id": 8,
            "title": "t",
            "state": "OPEN",
            "created_on": "2026-04-14T12:00:00+00:00",
            "updated_on": "2026-04-14T12:00:00+00:00",
            "draft": true,
            "destination": {"repository": {"full_name": "myorg/api"}},
            "links": {"html": {"href": "x"}},
            "participants": [
                {"role": "REVIEWER", "approved": false, "state": "changes_requested"},
            ],
        });
        let p = parse_pr(&body, "myorg").unwrap();
        // Draft trumps changes_requested.
        assert_eq!(p.status, Some(crate::models::PrStatus::Draft));
    }

    #[test]
    fn status_ready_when_approved_and_no_unreplied() {
        let mut p = pr("OPEN", "2026-04-14T12:00:00+00:00", 1);
        p.unreplied_comments = 0;
        p.status = p.derive_status();
        assert_eq!(p.status, Some(crate::models::PrStatus::ReadyToMerge));
    }

    #[test]
    fn status_needs_reply_beats_ready() {
        let mut p = pr("OPEN", "2026-04-14T12:00:00+00:00", 2);
        p.unreplied_comments = 1;
        p.status = p.derive_status();
        assert_eq!(p.status, Some(crate::models::PrStatus::NeedsReply));
    }

    #[test]
    fn status_needs_review_default() {
        let p = pr("OPEN", "2026-04-14T12:00:00+00:00", 0);
        assert_eq!(p.status, Some(crate::models::PrStatus::NeedsReview));
    }

    #[test]
    fn status_none_for_completed() {
        let p = pr("MERGED", "2026-04-14T12:00:00+00:00", 2);
        assert!(p.status.is_none());
    }

    #[test]
    fn tally_unreplied_counts_only_top_level_threads_without_replies() {
        let comments = vec![
            // Top-level by someone else, no reply → counts.
            json!({"id": 1, "user": {"uuid": "{other}"}}),
            // Top-level by someone else, has reply → doesn't count.
            json!({"id": 2, "user": {"uuid": "{other}"}}),
            json!({"id": 3, "parent": {"id": 2}, "user": {"uuid": "{me}"}}),
            // Top-level by author → excluded.
            json!({"id": 4, "user": {"uuid": "{me}"}}),
            // Top-level by other, deleted → excluded.
            json!({"id": 5, "user": {"uuid": "{other}"}, "deleted": true}),
            // Top-level inline outdated → excluded.
            json!({"id": 6, "user": {"uuid": "{other}"}, "inline": {"outdated": true, "path": "a"}}),
            // Top-level inline not outdated → counts.
            json!({"id": 7, "user": {"uuid": "{other}"}, "inline": {"outdated": false, "path": "a"}}),
        ];
        let info = tally_unreplied(&comments, "{me}");
        assert_eq!(info.unreplied, 2);
    }

    #[test]
    fn tally_unreplied_empty_on_no_comments() {
        assert_eq!(tally_unreplied(&[], "{me}").unreplied, 0);
    }
}
