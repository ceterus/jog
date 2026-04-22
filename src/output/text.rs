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
use crate::models::{Activity, BitbucketActivity, Flow, KanbanStats, PullRequest, SprintStats, StandupData, TodayIssue};
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
            section_header(&flow_header(data.flow.as_ref().unwrap()), theme),
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
            let avail = target.saturating_sub(pad + 6); // two " │ " separators
            let c1 = (avail * 46 / 116).max(30);
            let c2 = (avail * 32 / 116).max(22);
            let c3 = avail.saturating_sub(c1 + c2).max(24);
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
        let fill = content_width
            .saturating_sub(static_chrome + title_w + tag_w + 2);
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
    let body = format!(" {} · {} → {}", data.since_label, data.start_date, data.end_datetime);
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
                let day_word = if s.days_remaining == 1 { "d" } else { "d" };
                Some(format!(
                    "{} · {}{} left",
                    s.name, s.days_remaining, day_word
                ))
            }
        }
        Some(Flow::Kanban(k)) => Some(format!("Kanban · {}d window", k.window_days)),
        None => None,
    }
}

fn flow_header(flow: &Flow) -> String {
    match flow {
        Flow::Sprint(s) => {
            if s.state == "closed" {
                format!(" ▸ {} · closed", s.name)
            } else {
                format!(" ▸ {} · {} / {} days", s.name, s.days_remaining, s.total_days)
            }
        }
        Flow::Kanban(k) => format!(" ▸ FLOW · last {} days", k.window_days),
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
            dim(&format!("No Jira activity since {}.", data.start_date), theme)
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
    rows.push(dim(&format!(" {}", hline(width.saturating_sub(1), theme.unicode)), theme));
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
    } else if status_lower.contains("to do") || status_lower == "open" || status_lower == "backlog" {
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

    // Line 2: status.
    let status_str = dim(&a.status, theme);
    rows.push(format!("     {}", status_str));

    // Transitions.
    for t in &a.transitions {
        let body = format!("     {} {}", symbol_transition(theme), t);
        let body = truncate(&body, width);
        rows.push(dim(&body, theme));
    }

    // Updated fields.
    if !a.updated_fields.is_empty() {
        let mut f = a.updated_fields.clone();
        f.sort();
        f.dedup();
        let joined = f.join(", ");
        let body = format!("     {} {}", symbol_field(theme), joined);
        let body = truncate(&body, width);
        rows.push(dim(&body, theme));
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

fn push_today_line(rows: &mut Vec<String>, t: &TodayIssue, theme: &Theme, width: usize) {
    let (icon_in, icon_todo, _done) = icons_ticket(theme);
    let status_lower = t.status.to_lowercase();
    let icon = if status_lower.contains("to do") || status_lower == "open" || status_lower == "backlog" {
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

fn push_pr_block(rows: &mut Vec<String>, kind: &str, pr: &PullRequest, theme: &Theme, width: usize) {
    let (icon, icon_str) = match (kind, pr.state.as_str()) {
        ("opened", _) => ("opened", yellow(&symbol_pr_open(theme), theme)),
        ("completed", "MERGED") => ("merged", green(&symbol_pr_merged(theme), theme)),
        ("completed", "DECLINED") => ("declined", red(&symbol_pr_declined(theme), theme)),
        ("awaiting", _) => ("waiting", yellow(&symbol_pr_wait(theme), theme)),
        _ => ("", symbol_pr_open(theme)),
    };
    let _ = icon;

    // Line 1: icon + !id + repo
    let repo_short = short_repo(&pr.repo);
    let head = format!(
        "   {}  {}  {}",
        icon_str,
        cyan(&format!("!{}", pr.id), theme),
        dim(&repo_short, theme)
    );
    rows.push(truncate(&head, width));

    // Line 2+: title (wrapped).
    for (i, line) in wrap(&pr.title, width.saturating_sub(5)).into_iter().enumerate() {
        let prefix = if i == 0 { "     " } else { "     " };
        rows.push(format!("{}{}", prefix, line));
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
        "opened" => {
            let appr = if pr.approvals == 0 {
                "0 reviews".to_string()
            } else {
                format!("{} approval{}", pr.approvals, if pr.approvals == 1 { "" } else { "s" })
            };
            format!("opened · {}", appr)
        }
        "completed" => {
            if pr.state == "MERGED" {
                "merged".to_string()
            } else {
                "declined".to_string()
            }
        }
        "awaiting" => {
            if pr.approvals == 0 {
                "awaiting · 0 approvals".to_string()
            } else {
                format!(
                    "awaiting · {} approval{}",
                    pr.approvals,
                    if pr.approvals == 1 { "" } else { "s" }
                )
            }
        }
        _ => String::new(),
    }
}

fn short_repo(repo: &str) -> String {
    // "workspace/repo-slug" → "repo-slug"
    repo.rsplit_once('/').map(|(_, r)| r.to_string()).unwrap_or_else(|| repo.to_string())
}

// --- Column 3: stats / flow ----------------------------------------------

fn build_stats_column(flow: &Flow, stats: StatsMode, theme: &Theme, width: usize) -> Vec<String> {
    match flow {
        Flow::Sprint(s) => build_sprint_column(s, stats, theme, width),
        Flow::Kanban(k) => build_kanban_column(k, stats, theme, width),
    }
}

fn build_sprint_column(s: &SprintStats, stats: StatsMode, theme: &Theme, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let bar_w = width.saturating_sub(5).min(22).max(10);

    // Issues (always shown, even in Summary mode).
    let issues_pct = if s.issues_total > 0 {
        s.issues_done as f64 / s.issues_total as f64
    } else {
        0.0
    };
    rows.push(format!(
        "   {}     {} / {}   {:.0}%",
        bold("Issues", theme),
        s.issues_done,
        s.issues_total,
        issues_pct * 100.0
    ));
    rows.push(format!("   {}", progress_bar(issues_pct, bar_w, theme)));

    if stats == StatsMode::Summary {
        rows.push(String::new());
        let day_word = if s.days_remaining == 1 { "day" } else { "days" };
        rows.push(dim(&format!("   {} {} remaining", s.days_remaining, day_word), theme));
        return rows;
    }

    // Points.
    rows.push(String::new());
    let points_pct = if s.points_total > 0.0 {
        s.points_done / s.points_total
    } else {
        0.0
    };
    rows.push(format!(
        "   {}     {} / {}   {:.0}%",
        bold("Points", theme),
        fmt_points(s.points_done),
        fmt_points(s.points_total),
        points_pct * 100.0
    ));
    rows.push(format!("   {}", progress_bar(points_pct, bar_w, theme)));

    // Velocity.
    if let Some(ppd) = s.points_per_day {
        rows.push(String::new());
        rows.push(format!("   {}   {:.1} pt/d", bold("Velocity", theme), ppd));
        let remaining = s.points_total - s.points_done;
        if s.days_remaining > 0 && remaining > 0.0 {
            let need = remaining / s.days_remaining as f64;
            let tag = if need > ppd * 1.25 {
                red(&format!("{:.1} pt/d", need), theme)
            } else {
                yellow(&format!("{:.1} pt/d", need), theme)
            };
            rows.push(format!("   {}       {}", dim("Need", theme), tag));
        }
    }

    // Cycle times.
    rows.push(String::new());
    rows.push(dim("   Cycle (avg, done)", theme));
    push_cycle(&mut rows, "In Progress", s.avg_in_progress_hours, theme);
    push_cycle(&mut rows, "In Review", s.avg_in_review_hours, theme);
    push_cycle(&mut rows, "QA", s.avg_qa_hours, theme);

    rows
}

fn build_kanban_column(k: &KanbanStats, stats: StatsMode, theme: &Theme, width: usize) -> Vec<String> {
    let _ = width;
    let mut rows: Vec<String> = Vec::new();
    rows.push(format!("   {}          {}", bold("WIP", theme), k.wip_total));
    if !k.wip_by_status.is_empty() {
        for (status, n) in &k.wip_by_status {
            rows.push(format!("     {}  {}", pad_right(status, 16), dim(&n.to_string(), theme)));
        }
    }

    rows.push(String::new());
    rows.push(format!("   {}   {} done", bold("Throughput", theme), k.throughput));
    if let Some(tpd) = k.throughput_per_day {
        rows.push(format!("   {}       {:.2}/day", dim("Per day", theme), tpd));
    }

    if stats == StatsMode::Summary {
        return rows;
    }

    rows.push(String::new());
    rows.push(dim("   Cycle (avg, done)", theme));
    push_cycle(&mut rows, "In Progress", k.avg_in_progress_hours, theme);
    push_cycle(&mut rows, "In Review", k.avg_in_review_hours, theme);
    push_cycle(&mut rows, "QA", k.avg_qa_hours, theme);

    rows
}

fn push_cycle(rows: &mut Vec<String>, label: &str, hours: Option<f64>, theme: &Theme) {
    let val = match hours {
        Some(h) => fmt_duration(h),
        None => "—".to_string(),
    };
    rows.push(format!(
        "     {}  {}",
        pad_right(label, 14),
        dim(&val, theme)
    ));
}

fn progress_bar(frac: f64, width: usize, theme: &Theme) -> String {
    let frac = frac.clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    let (fc, ec) = if theme.unicode { ("█", "░") } else { ("#", "-") };
    let bar = format!("{}{}", fc.repeat(filled), ec.repeat(empty));
    // Color the filled portion green if near/over complete, yellow otherwise.
    let filled_str = if frac >= 0.9 {
        green(&fc.repeat(filled), theme)
    } else {
        cyan(&fc.repeat(filled), theme)
    };
    let empty_str = dim(&ec.repeat(empty), theme);
    let _ = bar;
    format!("{}{}", filled_str, empty_str)
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
        (
            yellow("●", theme),
            dim("○", theme),
            green("✓", theme),
        )
    } else {
        (
            yellow("*", theme),
            dim("o", theme),
            green("v", theme),
        )
    }
}

fn icons_pr(theme: &Theme) -> (String, String, String) {
    if theme.unicode {
        (
            yellow("↑", theme),
            green("✓", theme),
            yellow("⧖", theme),
        )
    } else {
        (yellow("^", theme), green("v", theme), yellow("?", theme))
    }
}

fn symbol_pr_open(theme: &Theme) -> String {
    if theme.unicode { "↑".to_string() } else { "^".to_string() }
}
fn symbol_pr_merged(theme: &Theme) -> String {
    if theme.unicode { "✓".to_string() } else { "v".to_string() }
}
fn symbol_pr_declined(theme: &Theme) -> String {
    if theme.unicode { "✗".to_string() } else { "x".to_string() }
}
fn symbol_pr_wait(theme: &Theme) -> String {
    if theme.unicode { "⧖".to_string() } else { "?".to_string() }
}
fn symbol_transition(theme: &Theme) -> &'static str {
    if theme.unicode { "→" } else { "->" }
}
fn symbol_comment(theme: &Theme) -> &'static str {
    if theme.unicode { "✎" } else { "~" }
}
fn symbol_field(theme: &Theme) -> &'static str {
    if theme.unicode { "⊕" } else { "+" }
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
        let head = flow_header(data.flow.as_ref().unwrap());
        print_section_header(&head, section_width, theme);
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
                let mut f = a.updated_fields.clone();
                f.sort();
                f.dedup();
                println!("      - updated: {}", f.join(", "));
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
    let approvals_note = if pr.state == "OPEN" {
        if pr.approvals == 0 {
            " (no approvals yet)".to_string()
        } else {
            format!(
                " ({} approval{})",
                pr.approvals,
                if pr.approvals == 1 { "" } else { "s" }
            )
        }
    } else {
        String::new()
    };
    println!(
        "    • !{} [{}] {}{}",
        pr.id, pr.repo, pr.title, approvals_note
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

#[cfg(test)]
mod smoke {
    //! Visual smoke tests — run with `cargo test -- --nocapture` to eyeball
    //! the landscape layout against a synthesised StandupData.

    use super::*;
    use crate::models::{Activity, BitbucketActivity, Flow, PullRequest, SprintStats, TodayIssue};
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
                updated_fields: vec!["story_points".into(), "sprint".into()],
                assigned_to_me: true,
            },
        );
        activities.insert(
            "PROJ-389".into(),
            Activity {
                summary: "Idempotency keys on /charge".into(),
                status: "Done".into(),
                transitions: vec!["In Review → Done".into()],
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
                updated_fields: vec!["description".into()],
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
