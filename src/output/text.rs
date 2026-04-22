//! Landscape "briefing card" renderer for the standup summary.
//!
//! Layout shape (120-col target):
//!
//! ```text
//! ╭─ Standup · <name> ───── <date> · <sprint/kanban tag> ─╮
//! │  since <date> · N events · project <keys> · board <id> │
//! ╰────────────────────────────────────────────────────────╯
//!
//!  ▸ YESTERDAY · N         │ ▸ PULL REQUESTS  │ ▸ SPRINT / FLOW
//!    ticket body           │   PR body        │   stats body
//!  ▸ TODAY · N             │                  │
//!    today body            │                  │
//!
//!  ─────────────────────────────────────────────────────────
//!   legend                                       version · cfg
//! ```
//!
//! Below 80 columns we switch to a stacked single-column card (same
//! styling, sections flow vertically). The legacy single-column plain
//! text output is only rendered when the user explicitly opts in via
//! `--plain` or `[output].layout = "plain"`.

use crate::comments::clean_comment;
use crate::config::{LayoutMode, StatsMode};
use crate::models::{
    Activity, BitbucketActivity, FieldChange, Flow, KanbanStats, PrStatus, PullRequest,
    SprintStats, StandupData, TodayIssue,
};
use crate::output::layout::{display_width, hline, pad_right, truncate, wrap, zip_columns};
use crate::output::theme::{bold, cyan, dim, green, red, yellow, Theme, STACKED_CUTOFF};

pub fn render(data: &StandupData, stats: StatsMode, layout: LayoutMode) {
    let theme = Theme::detect();
    // Plain mode is an explicit opt-out — always render the legacy plain
    // output regardless of terminal width. Card layout auto-adapts:
    // wide terminals get the tri-column landscape card; narrow terminals
    // get the same card styling with sections stacked vertically.
    match layout {
        LayoutMode::Plain => render_plain(data, stats, &theme),
        LayoutMode::Stacked => render_stacked(data, stats, &theme),
        LayoutMode::Card if theme.width < STACKED_CUTOFF => render_stacked(data, stats, &theme),
        LayoutMode::Card => render_landscape(data, stats, &theme),
    }
}

// ==========================================================================
// Landscape layout
// ==========================================================================

fn render_landscape(data: &StandupData, stats: StatsMode, theme: &Theme) {
    let show_stats = stats != StatsMode::Off && data.flow.is_some();
    let show_pr = data
        .bitbucket
        .as_ref()
        .map(|b| !b.is_empty())
        .unwrap_or(false);

    let (widths, sep_count) = column_widths(theme.width, show_pr, show_stats);
    let content_width: usize = widths.iter().sum::<usize>() + sep_count * 3;

    print_header(data, theme, content_width);

    // Build the parallel column buffers.
    let col_activity = build_activity_column(data, theme, widths[0]);
    let mut cols: Vec<Vec<String>> = vec![col_activity];
    let mut col_widths: Vec<usize> = vec![widths[0]];
    let mut idx = 1;
    if show_pr {
        cols.push(build_pr_column(
            data.bitbucket.as_ref().unwrap(),
            theme,
            widths[idx],
        ));
        col_widths.push(widths[idx]);
        idx += 1;
    }
    if show_stats {
        cols.push(build_stats_column(
            data.flow.as_ref().unwrap(),
            stats,
            theme,
            widths[idx],
        ));
        col_widths.push(widths[idx]);
    }

    // Column headers, then divider, then zipped body.
    let mut headers: Vec<Vec<String>> = Vec::with_capacity(cols.len());
    let yesterday_label = format!(" ▸ YESTERDAY · {} tickets", data.activities.len());
    headers.push(vec![
        section_header(&yesterday_label, theme),
        section_divider(widths[0], theme),
    ]);
    let mut i = 1;
    if show_pr {
        headers.push(vec![
            section_header(" ▸ PULL REQUESTS", theme),
            section_divider(widths[i], theme),
        ]);
        i += 1;
    }
    if show_stats {
        headers.push(vec![
            section_header(" ▸ STATS", theme),
            section_divider(widths[i], theme),
        ]);
    }

    for line in zip_columns(&headers, &col_widths, theme.unicode) {
        println!("{}", line);
    }
    for line in zip_columns(&cols, &col_widths, theme.unicode) {
        // Trim trailing spaces to keep output copy/paste friendly.
        println!("{}", line.trim_end());
    }

    println!();
    print_footer(theme, content_width);
}

/// Compute per-column widths based on available terminal width and which
/// panels are visible. Returns (widths, number-of-separators).
fn column_widths(term_width: usize, show_pr: bool, show_stats: bool) -> (Vec<usize>, usize) {
    let target = term_width.min(140);
    // 1 leading space on the left, no trailing pad — trim_end handles it.
    let pad = 1usize;
    match (show_pr, show_stats) {
        (true, true) => {
            // Stats panel has a low, fixed content ceiling (labels + small
            // numbers + a short progress bar), so cap it tight and give the
            // slack to the PR panel — PR titles and status badges wrap
            // otherwise. Visually this also right-aligns the stats block
            // against the card's right border.
            let avail = target.saturating_sub(pad + 6); // two " │ " separators
                                                        // Stats panel is right-aligned ledger rows; 22 cols comfortably
                                                        // fits "Vel 1.3 · Need 4.6" which is our widest natural row.
            let c3 = 22usize.min(avail / 3);
            let remaining = avail.saturating_sub(c3);
            let c1 = (remaining * 55 / 100).max(30);
            let c2 = remaining.saturating_sub(c1).max(28);
            (vec![c1, c2, c3], 2)
        }
        (true, false) => {
            let avail = target.saturating_sub(pad + 3); // one separator
            let c1 = (avail * 60 / 100).max(40);
            let c2 = avail.saturating_sub(c1).max(30);
            (vec![c1, c2], 1)
        }
        (false, true) => {
            let avail = target.saturating_sub(pad + 3);
            let c1 = (avail * 62 / 100).max(40);
            let c2 = avail.saturating_sub(c1).max(24);
            (vec![c1, c2], 1)
        }
        (false, false) => (vec![target.saturating_sub(pad)], 0),
    }
}

// --- Header / footer ------------------------------------------------------

fn print_header(data: &StandupData, theme: &Theme, content_width: usize) {
    let (tl, tr, ml, mr, bl, br, h) = if theme.unicode {
        ("╭", "╮", "│", "│", "╰", "╯", "─")
    } else {
        ("+", "+", "|", "|", "+", "+", "-")
    };

    // Top row: "╭─ Title ── … ── Tag ─╮" (or "╭─ Title ─╮" with no tag).
    let title_plain = format!("Standup · {}", data.user_name);
    let title = bold(&title_plain, theme);
    let tag_plain = flow_tag(data).unwrap_or_default();
    let tag = dim(&tag_plain, theme);
    let title_w = display_width(&title_plain);
    let tag_w = display_width(&tag_plain);
    let (left_pad, right_pad) = (format!("{}{} ", tl, h), format!(" {}{}", h, tr));
    let static_chrome = display_width(&left_pad) + display_width(&right_pad);
    if tag_w == 0 {
        // Corners + "─ title " + fill dashes.
        let fill = content_width.saturating_sub(static_chrome + title_w);
        println!("{}{} {}{}", left_pad, title, h.repeat(fill), right_pad);
    } else {
        // "─ title " + fill dashes + " tag ─"
        let fill = content_width.saturating_sub(static_chrome + title_w + tag_w + 2);
        println!(
            "{}{} {} {}{}",
            left_pad,
            title,
            h.repeat(fill),
            tag,
            right_pad
        );
    }

    // Middle row: "│ since … → …                          │"
    let inner = content_width.saturating_sub(2);
    let body = format!(
        " {} · {} → {}",
        data.since_label, data.start_date, data.end_datetime
    );
    let padded = pad_right(&body, inner);
    println!("{}{}{}", ml, dim(&padded, theme), mr);

    // Bottom row.
    println!("{}{}{}", bl, h.repeat(content_width.saturating_sub(2)), br);
    println!();
}

fn print_footer(theme: &Theme, content_width: usize) {
    let rule = hline(content_width, theme.unicode);
    println!("{}", dim(&rule, theme));
    let (icon_in, icon_todo, icon_done) = icons_ticket(theme);
    let (icon_open, icon_merged, icon_wait) = icons_pr(theme);
    let legend = format!(
        "  {} in-flight   {} todo   {} done      {} opened  {} merged  {} waiting",
        icon_in, icon_todo, icon_done, icon_open, icon_merged, icon_wait
    );
    let version = format!("jog v{}", env!("CARGO_PKG_VERSION"));
    let right = dim(&version, theme);
    let legend_w = display_width(&legend);
    let right_w = display_width(&right);
    let gap = content_width.saturating_sub(legend_w + right_w);
    println!("{}{}{}", legend, " ".repeat(gap), right);
}

fn flow_tag(data: &StandupData) -> Option<String> {
    match &data.flow {
        Some(Flow::Sprint(s)) => {
            if s.state == "closed" {
                Some(format!("{} (closed)", s.name))
            } else {
                Some(format!("{} · {}d left", s.name, s.days_remaining))
            }
        }
        Some(Flow::Kanban(k)) => Some(format!("Kanban · {}d window", k.window_days)),
        None => None,
    }
}

fn section_header(s: &str, theme: &Theme) -> String {
    bold(s, theme)
}

fn section_divider(width: usize, theme: &Theme) -> String {
    // Leading space keeps it flush with headers that also indent one.
    let line = format!(" {}", hline(width.saturating_sub(1), theme.unicode));
    dim(&line, theme)
}

// --- Column 1: activity ---------------------------------------------------

fn build_activity_column(data: &StandupData, theme: &Theme, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();

    if data.activities.is_empty() {
        rows.push(indent(1));
        rows.push(format!(
            "   {}",
            dim(
                &format!("No Jira activity since {}.", data.start_date),
                theme
            )
        ));
    } else {
        let items: Vec<(&String, &Activity)> = data.activities.iter().collect();
        for (i, (key, a)) in items.iter().enumerate() {
            push_activity_block(&mut rows, key, a, theme, width);
            if i + 1 < items.len() {
                rows.push(String::new());
            }
        }
    }

    // TODAY header directly below activities.
    rows.push(String::new());
    let today_label = format!(" ▸ TODAY · {} tickets", data.today.len());
    rows.push(bold(&today_label, theme));
    rows.push(dim(
        &format!(" {}", hline(width.saturating_sub(1), theme.unicode)),
        theme,
    ));
    if data.today.is_empty() {
        rows.push(format!(
            "   {}",
            dim("No in-progress issues assigned.", theme)
        ));
    } else {
        for t in &data.today {
            push_today_line(&mut rows, t, theme, width);
        }
    }

    rows
}

fn push_activity_block(
    rows: &mut Vec<String>,
    key: &str,
    a: &Activity,
    theme: &Theme,
    width: usize,
) {
    let (icon_in, icon_todo, icon_done) = icons_ticket(theme);
    let status_lower = a.status.to_lowercase();
    let icon = if status_lower.contains("done") || status_lower.contains("closed") {
        icon_done
    } else if status_lower.contains("to do") || status_lower == "open" || status_lower == "backlog"
    {
        icon_todo
    } else {
        icon_in
    };

    // Line 1: icon + key + summary (truncated to remaining width).
    let prefix = format!("   {}  {}  ", icon, cyan(key, theme));
    let prefix_w = display_width(&prefix);
    let summary_budget = width.saturating_sub(prefix_w);
    let summary = truncate(&a.summary, summary_budget);
    rows.push(format!("{}{}", prefix, bold(&summary, theme)));

    // Line 2+: lane chips — a compact chain showing every lane the ticket
    // visited in the window, with the current lane highlighted and any
    // backward move (bounced to a lane already visited) marked with ↩.
    // Replaces the old "status line + arrow list" pair.
    let chain = build_lane_chain(&a.transitions, &a.status);
    if !chain.is_empty() {
        let chips: Vec<String> = chain
            .iter()
            .enumerate()
            .map(|(i, step)| render_lane_chip(step, i + 1 == chain.len(), theme))
            .collect();
        let chip_budget = width.saturating_sub(5);
        for line in wrap_chips(&chips, chip_budget) {
            rows.push(format!("     {}", line));
        }
    }

    // Updated fields — show actual values (aliased, truncated for long
    // text), collapsed first→last. See `format_field_change` for rules.
    if !a.updated_fields.is_empty() {
        let formatted: Vec<String> = a.updated_fields.iter().map(format_field_change).collect();
        let glyph = symbol_field(theme);
        let prefix = format!("     {} ", glyph);
        let budget = width.saturating_sub(display_width(&prefix));
        for line in wrap_field_changes(&formatted, budget) {
            let body = format!("{}{}", prefix, line);
            rows.push(dim(&truncate(&body, width), theme));
        }
    }

    // Comments.
    for c in &a.my_comments {
        if let Some(clean) = clean_comment(c) {
            let body = format!("     {} \"{}\"", symbol_comment(theme), clean);
            let body = truncate(&body, width);
            rows.push(dim(&body, theme));
        }
    }
}

/// One lane in the visited-order chain. `backward=true` means this lane
/// was already present earlier in the chain — i.e. the ticket bounced
/// back to rework (e.g. In Review → In Progress).
#[derive(Debug, Clone, PartialEq, Eq)]
struct LaneStep {
    name: String,
    backward: bool,
}

/// Build the ordered list of lanes the ticket visited, derived from the
/// transition strings (`"A → B"`) plus the ticket's current status as a
/// fallback when there are no transitions in the window.
///
/// A lane is marked `backward` if it reappears after already being in
/// the chain — that's the signal we surface with a `↩` in the rendered
/// chip row.
fn build_lane_chain(transitions: &[String], current_status: &str) -> Vec<LaneStep> {
    let pairs: Vec<(String, String)> = transitions
        .iter()
        .filter_map(|s| {
            let (a, b) = s.split_once(" → ")?;
            Some((a.trim().to_string(), b.trim().to_string()))
        })
        .collect();
    if pairs.is_empty() {
        if current_status.is_empty() {
            return Vec::new();
        }
        return vec![LaneStep {
            name: current_status.to_string(),
            backward: false,
        }];
    }
    let mut chain = vec![LaneStep {
        name: pairs[0].0.clone(),
        backward: false,
    }];
    for (_, to) in &pairs {
        let backward = chain
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(to.as_str()));
        chain.push(LaneStep {
            name: to.clone(),
            backward,
        });
    }
    chain
}

/// Render one chip: `█ Done` for current (green, bold), `░ In Progress`
/// for past (yellow), optional leading `↩ ` when the step is a backward
/// move. Uses ASCII fallbacks when the theme lacks unicode.
fn render_lane_chip(step: &LaneStep, is_current: bool, theme: &Theme) -> String {
    let (filled, empty, back) = if theme.unicode {
        ("█", "░", "↩ ")
    } else {
        ("#", "-", "<- ")
    };
    let prefix = if step.backward { back } else { "" };
    let glyph = if is_current { filled } else { empty };
    let label = format!("{}{} {}", prefix, glyph, step.name);
    if is_current {
        bold(&green(&label, theme), theme)
    } else {
        yellow(&label, theme)
    }
}

/// Canonical alias for a Jira field name — strips `customfield_…`
/// noise, shortens verbose built-ins (`Story point estimate` →
/// `points`, `Fix Version/s` → `fixVersion`) and leaves anything we
/// don't recognise as-is. Case-insensitive match.
fn field_alias(raw: &str) -> String {
    let key = raw.trim().to_lowercase();
    match key.as_str() {
        "story point estimate" | "story points" | "storypoints" => "points".to_string(),
        "priority" => "priority".to_string(),
        "resolution" => "resolution".to_string(),
        "assignee" => "assignee".to_string(),
        "sprint" => "sprint".to_string(),
        "labels" => "labels".to_string(),
        "components" => "components".to_string(),
        "fix version/s" | "fix versions" | "fixversion" | "fixversions" => "fixVersion".to_string(),
        "epic link" => "epic".to_string(),
        "description" => "description".to_string(),
        "summary" => "summary".to_string(),
        "attachment" => "attachment".to_string(),
        _ => raw.trim().to_string(),
    }
}

/// True for fields whose values are multi-line prose — we never try to
/// render their contents inline; we just note that they changed.
fn is_long_text_field(alias: &str) -> bool {
    matches!(alias, "description" | "summary" | "comment")
}

/// Render one collapsed field change into a compact inline token, e.g.
/// `points: 3 → 5`, `+ sprint: Sprint 42`, `- assignee`,
/// `description: (updated)`. The summary field gets the long-text
/// treatment only when the new title is visibly long — short renames
/// still show the new title inline so the user sees the change.
pub fn format_field_change_public(c: &FieldChange) -> String {
    format_field_change(c)
}

fn format_field_change(c: &FieldChange) -> String {
    let alias = field_alias(&c.field);
    if is_long_text_field(&alias) {
        // Allow a short summary rename to show through; description is
        // always "(updated)".
        if alias == "summary" && !c.to.is_empty() && c.to.chars().count() <= 40 {
            return format!("summary: → {}", c.to);
        }
        return format!("{}: (updated)", alias);
    }
    match (c.from.is_empty(), c.to.is_empty()) {
        (true, true) => alias,
        (true, false) => format!("+ {}: {}", alias, c.to),
        (false, true) => format!("- {}", alias),
        (false, false) => format!("{}: {} → {}", alias, c.from, c.to),
    }
}

/// Wrap already-formatted field tokens onto multiple lines using ` · `
/// as a separator. Mirrors `wrap_chips` but with a single-char divider.
fn wrap_field_changes(tokens: &[String], width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    const SEP: &str = "  ·  ";
    let sep_w = display_width(SEP);
    for tok in tokens {
        let w = display_width(tok);
        let add = if current.is_empty() { w } else { sep_w + w };
        if !current.is_empty() && current_w + add > width {
            lines.push(std::mem::take(&mut current));
            current_w = 0;
        }
        if !current.is_empty() {
            current.push_str(SEP);
            current_w += sep_w;
        }
        current.push_str(tok);
        current_w += w;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Wrap a row of pre-rendered chips onto multiple lines so the row
/// respects the column width. Separator between chips is two spaces.
fn wrap_chips(chips: &[String], width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    const SEP: &str = "  ";
    const SEP_W: usize = 2;
    for chip in chips {
        let w = display_width(chip);
        let add = if current.is_empty() { w } else { SEP_W + w };
        if !current.is_empty() && current_w + add > width {
            lines.push(std::mem::take(&mut current));
            current_w = 0;
        }
        if !current.is_empty() {
            current.push_str(SEP);
            current_w += SEP_W;
        }
        current.push_str(chip);
        current_w += w;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn push_today_line(rows: &mut Vec<String>, t: &TodayIssue, theme: &Theme, width: usize) {
    let (icon_in, icon_todo, _done) = icons_ticket(theme);
    let status_lower = t.status.to_lowercase();
    let icon =
        if status_lower.contains("to do") || status_lower == "open" || status_lower == "backlog" {
            icon_todo
        } else {
            icon_in
        };
    let prefix = format!("   {}  {}  ", icon, cyan(&t.key, theme));
    let prefix_w = display_width(&prefix);
    let summary_budget = width.saturating_sub(prefix_w);
    let summary = truncate(&t.summary, summary_budget);
    rows.push(format!("{}{}", prefix, bold(&summary, theme)));
    rows.push(format!("     {}", dim(&t.status, theme)));
}

// --- Column 2: pull requests ---------------------------------------------

fn build_pr_column(bb: &BitbucketActivity, theme: &Theme, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let groups = [
        ("opened", bb.opened.as_slice()),
        ("completed", bb.completed.as_slice()),
        ("awaiting", bb.awaiting_approval.as_slice()),
    ];
    let mut first = true;
    for (kind, prs) in groups {
        for pr in prs {
            if !first {
                rows.push(String::new());
            }
            push_pr_block(&mut rows, kind, pr, theme, width);
            first = false;
        }
    }
    if rows.is_empty() {
        rows.push(format!("   {}", dim("No PR activity in window.", theme)));
    }
    rows
}

fn push_pr_block(
    rows: &mut Vec<String>,
    kind: &str,
    pr: &PullRequest,
    theme: &Theme,
    width: usize,
) {
    let (icon, icon_str) = match (kind, pr.state.as_str()) {
        ("opened", _) => ("opened", yellow(&symbol_pr_open(theme), theme)),
        ("completed", "MERGED") => ("merged", green(&symbol_pr_merged(theme), theme)),
        ("completed", "DECLINED") => ("declined", red(&symbol_pr_declined(theme), theme)),
        ("awaiting", _) => ("waiting", yellow(&symbol_pr_wait(theme), theme)),
        _ => ("", symbol_pr_open(theme)),
    };
    let _ = icon;

    // Line 1: icon + !id + repo + optional status badge
    let repo_short = short_repo(&pr.repo);
    let badge = pr
        .status
        .as_ref()
        .map(|s| format!(" {}", colored_badge(s, theme)))
        .unwrap_or_default();
    let head = format!(
        "   {}  {}  {}{}",
        icon_str,
        cyan(&format!("!{}", pr.id), theme),
        dim(&repo_short, theme),
        badge,
    );
    rows.push(truncate(&head, width));

    // Line 2+: title (wrapped).
    for line in wrap(&pr.title, width.saturating_sub(5)) {
        rows.push(format!("     {}", line));
    }

    // Line last: meta (status / approvals / age).
    let meta = pr_meta_line(kind, pr);
    if !meta.is_empty() {
        let body = format!("     {}", meta);
        rows.push(dim(&truncate(&body, width), theme));
    }
}

fn pr_meta_line(kind: &str, pr: &PullRequest) -> String {
    match kind {
        "opened" | "awaiting" => {
            let kind_label = if kind == "opened" {
                "opened"
            } else {
                "awaiting"
            };
            format!("{} · {}", kind_label, review_summary(pr))
        }
        "completed" => {
            if pr.state == "MERGED" {
                "merged".to_string()
            } else {
                "declined".to_string()
            }
        }
        _ => String::new(),
    }
}

/// Compact review summary for an OPEN PR meta line, e.g.
/// `2/3 approved · 1 unreplied`. Reviewer count is dropped when zero so
/// the line stays tight on workspaces that don't use explicit reviewers.
fn review_summary(pr: &PullRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    if pr.reviewers > 0 {
        parts.push(format!("{}/{} approved", pr.approvals, pr.reviewers));
    } else {
        let suffix = if pr.approvals == 1 { "" } else { "s" };
        parts.push(format!("{} approval{}", pr.approvals, suffix));
    }
    if pr.unreplied_comments > 0 {
        parts.push(format!("{} unreplied", pr.unreplied_comments));
    }
    parts.join(" · ")
}

/// Colored inline badge for a PR status. Red for blockers, yellow for
/// asks, green for greenlights, dim for draft/default review.
fn colored_badge(status: &PrStatus, theme: &Theme) -> String {
    let label = format!("[{}]", status.label());
    match status {
        PrStatus::ChangesRequested => red(&label, theme),
        PrStatus::NeedsReply => yellow(&label, theme),
        PrStatus::ReadyToMerge => green(&label, theme),
        PrStatus::Draft => dim(&label, theme),
        PrStatus::NeedsReview => dim(&label, theme),
    }
}

fn short_repo(repo: &str) -> String {
    // "workspace/repo-slug" → "repo-slug"
    repo.rsplit_once('/')
        .map(|(_, r)| r.to_string())
        .unwrap_or_else(|| repo.to_string())
}

// --- Column 3: stats / flow ----------------------------------------------

fn build_stats_column(flow: &Flow, stats: StatsMode, theme: &Theme, width: usize) -> Vec<String> {
    match flow {
        Flow::Sprint(s) => build_sprint_column(s, stats, theme, width),
        Flow::Kanban(k) => build_kanban_column(k, stats, theme, width),
    }
}

fn build_sprint_column(
    s: &SprintStats,
    stats: StatsMode,
    theme: &Theme,
    width: usize,
) -> Vec<String> {
    // Dot-leader ledger: `Label ···· value` rows, interspersed with
    // full-width progress bars and a sub-divider for cycle times. Labels
    // left-aligned, values flush with the column's right edge.
    let mut rows: Vec<String> = Vec::new();

    // Issues row + bar.
    let issues_pct = if s.issues_total > 0 {
        s.issues_done as f64 / s.issues_total as f64
    } else {
        0.0
    };
    rows.push(ledger_row(
        "Issues",
        &format!("{}/{}", s.issues_done, s.issues_total),
        width,
        theme,
    ));
    rows.push(bar_row(issues_pct, width, theme));

    if stats == StatsMode::Summary {
        let day_word = if s.days_remaining == 1 { "day" } else { "days" };
        rows.push(ledger_row(
            "Remaining",
            &format!("{} {}", s.days_remaining, day_word),
            width,
            theme,
        ));
        return rows;
    }

    // Points row + bar.
    let points_pct = if s.points_total > 0.0 {
        s.points_done / s.points_total
    } else {
        0.0
    };
    rows.push(ledger_row(
        "Points",
        &format!(
            "{}/{}",
            fmt_points(s.points_done),
            fmt_points(s.points_total)
        ),
        width,
        theme,
    ));
    rows.push(bar_row(points_pct, width, theme));

    // Pace: derived from current velocity vs required velocity.
    if let Some(ppd) = s.points_per_day {
        if s.days_remaining > 0 {
            let remaining = s.points_total - s.points_done;
            if remaining > 0.0 {
                let need = remaining / s.days_remaining as f64;
                let delta = (ppd - need) * s.days_remaining as f64;
                let (text, paint): (String, fn(&str, &Theme) -> String) = if delta >= -0.5 {
                    (format!("on pace · {:+.0}", delta), green)
                } else {
                    (format!("behind {:.0}", -delta), red)
                };
                rows.push(ledger_row_styled("Pace", &text, width, theme, Some(paint)));
            }
        }
        rows.push(ledger_row(
            "Velocity",
            &format!("{:.1}/day", ppd),
            width,
            theme,
        ));
    }

    // Cycle sub-section.
    rows.push(sub_divider("Cycle (avg)", width, theme));
    rows.push(ledger_row_opt(
        "Resolve",
        s.avg_resolve_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "Todo→Done",
        s.avg_todo_to_done_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "Prog",
        s.avg_in_progress_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "Rev",
        s.avg_in_review_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "QA",
        s.avg_qa_hours.map(fmt_duration),
        width,
        theme,
    ));

    push_spark_section(
        &mut rows,
        &format!("Done / day ({}d)", s.done_per_day.len()),
        &s.done_per_day,
        width,
        theme,
    );

    rows
}

/// Append a sub-divider + sparkline row, but only if the series has at
/// least 3 data points and the column is wide enough to render it.
fn push_spark_section(
    rows: &mut Vec<String>,
    label: &str,
    series: &[u32],
    width: usize,
    theme: &Theme,
) {
    if series.len() < 3 || width < 10 {
        return;
    }
    rows.push(sub_divider(label, width, theme));
    rows.push(sparkline_row(series, width, theme));
}

/// Render a unicode sparkline (`▁▂▃▄▅▆▇█`) of up to `width` chars. Zero
/// buckets render as a space so empty days stay visually distinct from
/// "one done". Falls back to `.`/`#` for non-unicode themes.
fn sparkline_row(series: &[u32], width: usize, theme: &Theme) -> String {
    let sparks: &[&str] = if theme.unicode {
        &["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"]
    } else {
        &[".", ":", "-", "=", "+", "*", "#", "@"]
    };
    // If the series is wider than the column, drop oldest days so the
    // sparkline stays right-anchored to "today".
    let take = series.len().min(width);
    let start = series.len() - take;
    let slice = &series[start..];
    let max = *slice.iter().max().unwrap_or(&0);
    let mut out = String::new();
    for &v in slice {
        if v == 0 || max == 0 {
            out.push(' ');
        } else {
            let idx = ((v as usize * sparks.len()).saturating_sub(1)) / (max as usize);
            let idx = idx.min(sparks.len() - 1);
            out.push_str(sparks[idx]);
        }
    }
    // Left-pad so the sparkline hugs the right edge like other ledger rows.
    let w = display_width(&out);
    let pad = width.saturating_sub(w);
    format!("{}{}", " ".repeat(pad), cyan(&out, theme))
}

/// Build one dot-leader ledger row: `Label ······· value`.
///
/// The label is left-aligned and rendered bold; the value is flush with
/// the column's right edge. Dots fill the gap and are dimmed. Falls
/// back to ASCII `.` when the theme lacks unicode.
fn ledger_row(label: &str, value: &str, width: usize, theme: &Theme) -> String {
    ledger_row_styled(label, value, width, theme, None)
}

fn ledger_row_styled(
    label: &str,
    value: &str,
    width: usize,
    theme: &Theme,
    paint_value: Option<fn(&str, &Theme) -> String>,
) -> String {
    let dot = if theme.unicode { "·" } else { "." };
    let label_w = display_width(label);
    let value_w = display_width(value);
    // Reserve one space on each side of the dot run so labels and values
    // don't touch the leader.
    let inner = width.saturating_sub(label_w + value_w + 2);
    let dots = if inner == 0 { 1 } else { inner };
    let painted_value = match paint_value {
        Some(f) => f(value, theme),
        None => value.to_string(),
    };
    format!(
        "{} {} {}",
        bold(label, theme),
        dim(&dot.repeat(dots), theme),
        painted_value,
    )
}

/// Ledger row with an optional value — renders `—` when `None`.
fn ledger_row_opt(label: &str, value: Option<String>, width: usize, theme: &Theme) -> String {
    match value {
        Some(v) => ledger_row(label, &v, width, theme),
        None => ledger_row(label, "—", width, theme),
    }
}

/// Full-width progress bar row with trailing percentage, e.g.
/// `█████████░░░░░░░░ 54%`. Leaves 5 cells of headroom (` 100%`).
fn bar_row(frac: f64, width: usize, theme: &Theme) -> String {
    let frac = frac.clamp(0.0, 1.0);
    let pct = (frac * 100.0).round() as u64;
    // " 100%" = 5 chars; reserve that even when showing smaller numbers
    // so all bars line up regardless of current percentage.
    let reserve = 5usize;
    let bar_w = width.saturating_sub(reserve).max(4);
    let filled = (frac * bar_w as f64).round() as usize;
    let filled = filled.min(bar_w);
    let empty = bar_w - filled;
    let (fc, ec) = if theme.unicode {
        ("█", "░")
    } else {
        ("#", "-")
    };
    let filled_str = if frac >= 0.9 {
        green(&fc.repeat(filled), theme)
    } else {
        cyan(&fc.repeat(filled), theme)
    };
    let empty_str = dim(&ec.repeat(empty), theme);
    format!("{}{} {:>3}%", filled_str, empty_str, pct)
}

/// Sub-section divider inside the stats card, e.g. `── Cycle (avg) ──`.
/// Centres the label within a run of dim horizontal lines sized to the
/// column width.
fn sub_divider(label: &str, width: usize, theme: &Theme) -> String {
    let h = if theme.unicode { "─" } else { "-" };
    let label_padded = format!(" {} ", label);
    let label_w = display_width(&label_padded);
    let remaining = width.saturating_sub(label_w);
    let left = remaining / 2;
    let right = remaining - left;
    dim(
        &format!("{}{}{}", h.repeat(left), label_padded, h.repeat(right)),
        theme,
    )
}

fn build_kanban_column(
    k: &KanbanStats,
    stats: StatsMode,
    theme: &Theme,
    width: usize,
) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    rows.push(ledger_row("WIP", &k.wip_total.to_string(), width, theme));
    for (status, n) in &k.wip_by_status {
        // Indent sub-rows one space; the indent eats into the label
        // slot, leaving the value column intact.
        let label = format!(" {}", status);
        rows.push(ledger_row(&label, &n.to_string(), width, theme));
    }

    rows.push(ledger_row(
        "Throughput",
        &k.throughput.to_string(),
        width,
        theme,
    ));
    if let Some(tpd) = k.throughput_per_day {
        rows.push(ledger_row(" per day", &format!("{:.2}", tpd), width, theme));
    }

    if stats == StatsMode::Summary {
        return rows;
    }

    rows.push(sub_divider("Cycle (avg)", width, theme));
    rows.push(ledger_row_opt(
        "Resolve",
        k.avg_resolve_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "Todo→Done",
        k.avg_todo_to_done_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "Prog",
        k.avg_in_progress_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "Rev",
        k.avg_in_review_hours.map(fmt_duration),
        width,
        theme,
    ));
    rows.push(ledger_row_opt(
        "QA",
        k.avg_qa_hours.map(fmt_duration),
        width,
        theme,
    ));

    push_spark_section(
        &mut rows,
        &format!("Done / day ({}d)", k.done_per_day.len()),
        &k.done_per_day,
        width,
        theme,
    );

    rows
}

fn fmt_points(p: f64) -> String {
    if (p - p.round()).abs() < 0.05 {
        format!("{}", p.round() as i64)
    } else {
        format!("{:.1}", p)
    }
}

// --- Icon helpers --------------------------------------------------------

fn icons_ticket(theme: &Theme) -> (String, String, String) {
    if theme.unicode {
        (yellow("●", theme), dim("○", theme), green("✓", theme))
    } else {
        (yellow("*", theme), dim("o", theme), green("v", theme))
    }
}

fn icons_pr(theme: &Theme) -> (String, String, String) {
    if theme.unicode {
        (yellow("↑", theme), green("✓", theme), yellow("⧖", theme))
    } else {
        (yellow("^", theme), green("v", theme), yellow("?", theme))
    }
}

fn symbol_pr_open(theme: &Theme) -> String {
    if theme.unicode {
        "↑".to_string()
    } else {
        "^".to_string()
    }
}
fn symbol_pr_merged(theme: &Theme) -> String {
    if theme.unicode {
        "✓".to_string()
    } else {
        "v".to_string()
    }
}
fn symbol_pr_declined(theme: &Theme) -> String {
    if theme.unicode {
        "✗".to_string()
    } else {
        "x".to_string()
    }
}
fn symbol_pr_wait(theme: &Theme) -> String {
    if theme.unicode {
        "⧖".to_string()
    } else {
        "?".to_string()
    }
}
fn symbol_comment(theme: &Theme) -> &'static str {
    if theme.unicode {
        "✎"
    } else {
        "~"
    }
}
fn symbol_field(theme: &Theme) -> &'static str {
    if theme.unicode {
        "⊕"
    } else {
        "+"
    }
}

fn indent(n: usize) -> String {
    " ".repeat(n)
}

// ==========================================================================
// Stacked card layout (narrow TUI, < 80 cols).
//
// Same visual language as the landscape card — boxed header, coloured
// icons, `▸` section headers with dim dividers, progress bars, footer
// legend — but sections flow vertically instead of side-by-side.
// Reuses the same column builders as landscape; only the composition
// changes.
// ==========================================================================

fn render_stacked(data: &StandupData, stats: StatsMode, theme: &Theme) {
    let show_stats = stats != StatsMode::Off && data.flow.is_some();
    let show_pr = data
        .bitbucket
        .as_ref()
        .map(|b| !b.is_empty())
        .unwrap_or(false);

    // Cap the stacked card at 100 cols even on very wide terminals —
    // single-column text gets hard to read past that, and the user who
    // explicitly asked for `--stacked` on a 160-col monitor probably
    // doesn't want a 160-col-wide card.
    let content_width = theme.width.clamp(50, 100);
    // Section body builders indent their rows by one space already, so
    // we feed them a width one less than the content width to avoid
    // overflow when a line lands on the boundary.
    let section_width = content_width.saturating_sub(1);

    print_header(data, theme, content_width);

    // --- Activity (yesterday + today bundled) ---
    print_section_header(
        &format!(" ▸ YESTERDAY · {} tickets", data.activities.len()),
        section_width,
        theme,
    );
    for row in build_activity_column(data, theme, section_width) {
        println!("{}", row.trim_end());
    }

    // --- Pull requests ---
    if show_pr {
        println!();
        print_section_header(" ▸ PULL REQUESTS", section_width, theme);
        for row in build_pr_column(data.bitbucket.as_ref().unwrap(), theme, section_width) {
            println!("{}", row.trim_end());
        }
    }

    // --- Flow / stats ---
    if show_stats {
        println!();
        print_section_header(" ▸ STATS", section_width, theme);
        for row in build_stats_column(data.flow.as_ref().unwrap(), stats, theme, section_width) {
            println!("{}", row.trim_end());
        }
    }

    println!();
    print_footer(theme, content_width);
}

fn print_section_header(label: &str, width: usize, theme: &Theme) {
    println!("{}", bold(label, theme));
    println!("{}", section_divider(width, theme));
}

// ==========================================================================
// Plain layout — legacy single-column output. Only used when the user
// explicitly opts in via `--plain` / `--layout plain` / `[output].layout`.
// ==========================================================================

fn render_plain(data: &StandupData, stats: StatsMode, theme: &Theme) {
    let bar = if theme.unicode { "═" } else { "=" };
    let rule = bar.repeat(43);
    println!("{}", rule);
    println!(" Standup — {} ({})", data.user_name, data.end_datetime);
    println!("{}", rule);
    println!();
    println!("{} ({} → now):", data.since_label, data.start_date);
    if data.activities.is_empty() {
        println!("  • No Jira activity found since {}.", data.start_date);
    } else {
        for (key, a) in &data.activities {
            println!("  • [{}] {} (status: {})", key, a.summary, a.status);
            for t in &a.transitions {
                println!("      - transitioned: {}", t);
            }
            if !a.updated_fields.is_empty() {
                let f: Vec<String> = a.updated_fields.iter().map(format_field_change).collect();
                println!("      - updated: {}", f.join(" · "));
            }
            for c in &a.my_comments {
                if let Some(clean) = clean_comment(c) {
                    println!("      - commented: \"{}\"", clean);
                }
            }
        }
    }
    println!();
    println!("Today:");
    if data.today.is_empty() {
        println!("  • No in-progress issues assigned.");
    } else {
        for t in &data.today {
            println!("  • [{}] {} ({})", t.key, t.summary, t.status);
        }
    }
    if let Some(bb) = &data.bitbucket {
        println!();
        render_plain_bitbucket(bb);
    }
    if stats == StatsMode::Off {
        return;
    }
    println!();
    match &data.flow {
        Some(Flow::Sprint(s)) => render_plain_sprint(s, stats),
        Some(Flow::Kanban(k)) => render_plain_kanban(k, stats),
        None => {
            println!("Sprint:");
            println!("  No active sprint found.");
        }
    }
}

fn render_plain_bitbucket(bb: &BitbucketActivity) {
    println!("Bitbucket:");
    if !bb.opened.is_empty() {
        println!("  Opened:");
        for pr in &bb.opened {
            print_plain_pr(pr);
        }
    }
    if !bb.completed.is_empty() {
        let any_merged = bb.completed.iter().any(|p| p.state == "MERGED");
        let any_declined = bb.completed.iter().any(|p| p.state == "DECLINED");
        let head = match (any_merged, any_declined) {
            (true, true) => "Merged / declined",
            (true, false) => "Merged",
            (false, true) => "Declined",
            _ => "Completed",
        };
        println!("  {}:", head);
        for pr in &bb.completed {
            print_plain_pr(pr);
        }
    }
    if !bb.awaiting_approval.is_empty() {
        println!("  Awaiting approval:");
        for pr in &bb.awaiting_approval {
            print_plain_pr(pr);
        }
    }
}

fn print_plain_pr(pr: &PullRequest) {
    let badge = pr
        .status
        .as_ref()
        .map(|s| format!(" [{}]", s.label()))
        .unwrap_or_default();
    let note = if pr.state == "OPEN" {
        format!(" ({})", review_summary(pr))
    } else {
        String::new()
    };
    println!(
        "    •{} !{} [{}] {}{}",
        badge, pr.id, pr.repo, pr.title, note
    );
}

fn render_plain_sprint(s: &SprintStats, stats: StatsMode) {
    println!("Sprint:");
    if s.state == "closed" {
        let ended = (-s.days_remaining).max(0);
        let day_word = if ended == 1 { "day" } else { "days" };
        println!(
            "  {} (closed — ended {} {} ago, was {} days long)",
            s.name, ended, day_word, s.total_days
        );
    } else {
        let day_word = if s.days_remaining == 1 { "day" } else { "days" };
        println!(
            "  {} ({} {} left of {})",
            s.name, s.days_remaining, day_word, s.total_days
        );
    }
    println!("  Issues: {}/{} done", s.issues_done, s.issues_total);
    if stats == StatsMode::Summary {
        return;
    }
    println!(
        "  Points: {}/{} done ({:.0}%)",
        s.points_done,
        s.points_total,
        if s.points_total > 0.0 {
            s.points_done / s.points_total * 100.0
        } else {
            0.0
        }
    );
    println!();
    if let Some(ppd) = s.points_per_day {
        println!("  Velocity:");
        println!("    Current:  {:.1} pts/day", ppd);
        let points_remaining = s.points_total - s.points_done;
        if s.days_remaining > 0 && points_remaining > 0.0 {
            println!(
                "    Needed:   {:.1} pts/day to finish on time",
                points_remaining / s.days_remaining as f64
            );
        }
    }
    println!();
    println!("  Avg Cycle Times (completed tickets):");
    plain_cycle("Created → Done", s.avg_resolve_hours);
    plain_cycle("To Do → Done", s.avg_todo_to_done_hours);
    plain_cycle("In Progress", s.avg_in_progress_hours);
    plain_cycle("In Review", s.avg_in_review_hours);
    plain_cycle("QA", s.avg_qa_hours);
}

fn render_plain_kanban(k: &KanbanStats, stats: StatsMode) {
    println!("Flow (Kanban, last {} days):", k.window_days);
    println!("  WIP: {} open", k.wip_total);
    if stats == StatsMode::Summary {
        println!("  Throughput: {} issues done", k.throughput);
        return;
    }
    if !k.wip_by_status.is_empty() {
        for (status, n) in &k.wip_by_status {
            println!("    {:20} {}", status, n);
        }
    }
    println!();
    println!("  Throughput: {} issues done", k.throughput);
    if let Some(tpd) = k.throughput_per_day {
        println!("    {:.2} issues/day", tpd);
    }
    println!();
    println!("  Avg Cycle Times (completed tickets):");
    plain_cycle("Created → Done", k.avg_resolve_hours);
    plain_cycle("To Do → Done", k.avg_todo_to_done_hours);
    plain_cycle("In Progress", k.avg_in_progress_hours);
    plain_cycle("In Review", k.avg_in_review_hours);
    plain_cycle("QA", k.avg_qa_hours);
}

fn plain_cycle(label: &str, hours: Option<f64>) {
    let padded = format!("    {:20} ", label);
    match hours {
        Some(h) => println!("{}{}", padded, fmt_duration(h)),
        None => println!("{}—", padded),
    }
}

// --- Duration formatter (shared with markdown renderer) ------------------

pub fn fmt_duration(hours: f64) -> String {
    if hours < 1.0 {
        format!("{:.0}m", hours * 60.0)
    } else if hours < 24.0 {
        let h = hours.floor() as u64;
        let m = ((hours - h as f64) * 60.0).round() as u64;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h {}m", h, m)
        }
    } else {
        let d = (hours / 24.0).floor() as u64;
        let h = (hours - d as f64 * 24.0).round() as u64;
        if h == 0 {
            format!("{}d", d)
        } else {
            format!("{}d {}h", d, h)
        }
    }
}

#[cfg(test)]
mod field_change_tests {
    use super::*;

    fn c(field: &str, from: &str, to: &str) -> FieldChange {
        FieldChange {
            field: field.into(),
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn aliases_common_jira_names() {
        assert_eq!(field_alias("Story point estimate"), "points");
        assert_eq!(field_alias("story points"), "points");
        assert_eq!(field_alias("Fix Version/s"), "fixVersion");
        assert_eq!(field_alias("Epic Link"), "epic");
        assert_eq!(field_alias("resolution"), "resolution");
    }

    #[test]
    fn unknown_field_passes_through_trimmed() {
        assert_eq!(field_alias(" Some Custom Field "), "Some Custom Field");
    }

    #[test]
    fn formats_value_change() {
        assert_eq!(
            format_field_change(&c("Story point estimate", "3", "5")),
            "points: 3 → 5"
        );
    }

    #[test]
    fn formats_set_from_empty() {
        assert_eq!(
            format_field_change(&c("Sprint", "", "Sprint 42")),
            "+ sprint: Sprint 42"
        );
    }

    #[test]
    fn formats_clear_to_empty() {
        assert_eq!(
            format_field_change(&c("Assignee", "Jane", "")),
            "- assignee"
        );
    }

    #[test]
    fn long_description_renders_as_updated() {
        assert_eq!(
            format_field_change(&c("description", "old", "new")),
            "description: (updated)"
        );
    }

    #[test]
    fn short_summary_rename_shows_new_title() {
        let out = format_field_change(&c("summary", "A", "Fix retry on 425"));
        assert_eq!(out, "summary: → Fix retry on 425");
    }

    #[test]
    fn very_long_summary_falls_back_to_updated() {
        let long =
            "This is a very long summary that exceeds forty characters by a fair margin indeed";
        assert_eq!(
            format_field_change(&c("summary", "old", long)),
            "summary: (updated)"
        );
    }
}

#[cfg(test)]
mod lane_chain_tests {
    use super::*;

    #[test]
    fn empty_transitions_fall_back_to_current_status() {
        let chain = build_lane_chain(&[], "In Progress");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].name, "In Progress");
        assert!(!chain[0].backward);
    }

    #[test]
    fn empty_transitions_and_empty_status_returns_empty() {
        let chain = build_lane_chain(&[], "");
        assert!(chain.is_empty());
    }

    #[test]
    fn single_transition_produces_two_lane_chain() {
        let t = vec!["To Do → In Progress".to_string()];
        let chain = build_lane_chain(&t, "In Progress");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].name, "To Do");
        assert_eq!(chain[1].name, "In Progress");
        assert!(!chain[1].backward);
    }

    #[test]
    fn multi_transition_chain() {
        let t = vec![
            "To Do → In Progress".to_string(),
            "In Progress → Done".to_string(),
        ];
        let chain = build_lane_chain(&t, "Done");
        let names: Vec<&str> = chain.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["To Do", "In Progress", "Done"]);
        assert!(chain.iter().all(|s| !s.backward));
    }

    #[test]
    fn revisiting_a_lane_marks_it_backward() {
        let t = vec![
            "To Do → In Progress".to_string(),
            "In Progress → In Review".to_string(),
            "In Review → In Progress".to_string(),
            "In Progress → Done".to_string(),
        ];
        let chain = build_lane_chain(&t, "Done");
        assert_eq!(chain.len(), 5);
        // [To Do, In Progress, In Review, In Progress (back), Done]
        assert!(!chain[0].backward);
        assert!(!chain[1].backward);
        assert!(!chain[2].backward);
        assert!(chain[3].backward, "second In Progress should be backward");
        assert!(!chain[4].backward);
    }

    #[test]
    fn lane_comparison_is_case_insensitive() {
        let t = vec![
            "to do → in progress".to_string(),
            "in progress → In Progress".to_string(),
        ];
        let chain = build_lane_chain(&t, "In Progress");
        // Last step ("In Progress") matches "in progress" already in chain.
        assert!(chain.last().unwrap().backward);
    }
}

#[cfg(test)]
mod smoke {
    //! Visual smoke tests — run with `cargo test -- --nocapture` to eyeball
    //! the landscape layout against a synthesised StandupData.

    use super::*;
    use crate::models::{
        Activity, BitbucketActivity, FieldChange, Flow, PullRequest, SprintStats, TodayIssue,
    };
    use std::collections::BTreeMap;

    fn fixture() -> StandupData {
        let mut activities: BTreeMap<String, Activity> = BTreeMap::new();
        activities.insert(
            "PROJ-412".into(),
            Activity {
                summary: "Refund webhook retry logic".into(),
                status: "In Review".into(),
                transitions: vec!["In Progress → In Review".into()],
                my_comments: vec!["spec covers 409 but not 425 — added test".into()],
                updated_fields: vec![
                    FieldChange {
                        field: "Story point estimate".into(),
                        from: "3".into(),
                        to: "5".into(),
                    },
                    FieldChange {
                        field: "Sprint".into(),
                        from: "".into(),
                        to: "Sprint 42".into(),
                    },
                ],
                assigned_to_me: true,
            },
        );
        activities.insert(
            "PROJ-389".into(),
            Activity {
                summary: "Idempotency keys on /charge".into(),
                status: "Done".into(),
                transitions: vec![
                    "To Do → In Progress".into(),
                    "In Progress → In Review".into(),
                    "In Review → In Progress".into(),
                    "In Progress → Done".into(),
                ],
                my_comments: vec![],
                updated_fields: vec![],
                assigned_to_me: true,
            },
        );
        activities.insert(
            "PROJ-401".into(),
            Activity {
                summary: "Dashboard latency spike".into(),
                status: "In Progress".into(),
                transitions: vec![],
                my_comments: vec![],
                updated_fields: vec![FieldChange {
                    field: "description".into(),
                    from: "old blurb".into(),
                    to: "new blurb with many more details".into(),
                }],
                assigned_to_me: true,
            },
        );
        activities.insert(
            "PROJ-388".into(),
            Activity {
                summary: "OAuth refresh race condition".into(),
                status: "Done".into(),
                transitions: vec![],
                my_comments: vec![],
                updated_fields: vec![],
                assigned_to_me: true,
            },
        );

        let today = vec![
            TodayIssue {
                key: "PROJ-420".into(),
                summary: "Backfill legacy accounts".into(),
                status: "In Progress".into(),
            },
            TodayIssue {
                key: "PROJ-425".into(),
                summary: "Investigate 5xx in eu-west-1".into(),
                status: "To Do".into(),
            },
        ];

        let bitbucket = BitbucketActivity {
            opened: vec![PullRequest {
                id: 234,
                title: "Retry logic for webhook failures".into(),
                repo: "team/payments".into(),
                state: "OPEN".into(),
                url: String::new(),
                created_on: String::new(),
                updated_on: String::new(),
                approvals: 0,
                reviewers: 2,
                unreplied_comments: 0,
                changes_requested: false,
                is_draft: false,
                status: Some(crate::models::PrStatus::NeedsReview),
            }],
            completed: vec![PullRequest {
                id: 228,
                title: "Idempotency keys on /charge".into(),
                repo: "team/payments".into(),
                state: "MERGED".into(),
                url: String::new(),
                created_on: String::new(),
                updated_on: String::new(),
                approvals: 2,
                reviewers: 2,
                unreplied_comments: 0,
                changes_requested: false,
                is_draft: false,
                status: None,
            }],
            awaiting_approval: vec![PullRequest {
                id: 231,
                title: "Latency dashboard v2".into(),
                repo: "team/dashboards".into(),
                state: "OPEN".into(),
                url: String::new(),
                created_on: String::new(),
                updated_on: String::new(),
                approvals: 0,
                reviewers: 1,
                unreplied_comments: 2,
                changes_requested: false,
                is_draft: false,
                status: Some(crate::models::PrStatus::NeedsReply),
            }],
        };

        let sprint = SprintStats {
            name: "Sprint 42".into(),
            state: "active".into(),
            days_remaining: 3,
            total_days: 14,
            days_elapsed: 11,
            points_done: 18.0,
            points_total: 28.0,
            issues_done: 7,
            issues_total: 11,
            avg_resolve_hours: Some(52.0),
            avg_in_progress_hours: Some(8.0),
            avg_in_review_hours: Some(3.0),
            avg_qa_hours: Some(1.0),
            avg_todo_to_done_hours: Some(30.0),
            points_per_day: Some(1.6),
            done_per_day: vec![0, 1, 0, 2, 1, 3, 1, 2, 0, 1, 2, 1, 3, 2],
        };

        StandupData {
            user_name: "Anthony Norfleet".into(),
            start_date: "2026-04-21".into(),
            end_datetime: "2026-04-22 09:04".into(),
            since_label: "Since Tue Apr 21".into(),
            activities,
            today,
            flow: Some(Flow::Sprint(sprint)),
            bitbucket: Some(bitbucket),
        }
    }

    #[test]
    fn landscape_full() {
        let data = fixture();
        let theme = Theme::plain_landscape();
        println!("\n--- landscape, stats=full ---");
        render_landscape(&data, StatsMode::Full, &theme);
    }

    #[test]
    fn landscape_no_stats() {
        let data = fixture();
        let theme = Theme::plain_landscape();
        println!("\n--- landscape, stats=off ---");
        render_landscape(&data, StatsMode::Off, &theme);
    }

    #[test]
    fn landscape_summary() {
        let data = fixture();
        let theme = Theme::plain_landscape();
        println!("\n--- landscape, stats=summary ---");
        render_landscape(&data, StatsMode::Summary, &theme);
    }

    #[test]
    fn plain_layout_renders_legacy() {
        let data = fixture();
        let theme = Theme::plain_landscape();
        println!("\n--- plain (legacy single-column text) ---");
        render_plain(&data, StatsMode::Full, &theme);
    }

    #[test]
    fn stacked_card_full() {
        let data = fixture();
        let theme = Theme::plain(70);
        println!("\n--- stacked card (70 cols) ---");
        render_stacked(&data, StatsMode::Full, &theme);
    }

    #[test]
    fn stacked_card_no_stats() {
        let data = fixture();
        let theme = Theme::plain(70);
        println!("\n--- stacked card, stats=off (70 cols) ---");
        render_stacked(&data, StatsMode::Off, &theme);
    }
}
