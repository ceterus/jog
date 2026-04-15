use crate::comments::clean_comment;
use crate::config::StatsMode;
use crate::models::{BitbucketActivity, Flow, KanbanStats, PullRequest, SprintStats, StandupData};

pub fn render(data: &StandupData, stats: StatsMode) {
    println!("═══════════════════════════════════════════");
    println!(" Standup — {} ({})", data.user_name, data.end_datetime);
    println!("═══════════════════════════════════════════");
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
        render_bitbucket(bb);
    }
    if stats == StatsMode::Off {
        return;
    }
    println!();
    match &data.flow {
        Some(Flow::Sprint(s)) => render_sprint(s, stats),
        Some(Flow::Kanban(k)) => render_kanban(k, stats),
        None => {
            println!("Sprint:");
            println!("  No active sprint found.");
        }
    }
}

fn render_bitbucket(bb: &BitbucketActivity) {
    println!("Bitbucket:");
    if !bb.opened.is_empty() {
        println!("  Opened:");
        for pr in &bb.opened {
            print_pr(pr);
        }
    }
    if !bb.completed.is_empty() {
        println!("  {}:", completed_heading(&bb.completed));
        for pr in &bb.completed {
            print_pr(pr);
        }
    }
    if !bb.awaiting_approval.is_empty() {
        println!("  Awaiting approval:");
        for pr in &bb.awaiting_approval {
            print_pr(pr);
        }
    }
}

fn completed_heading(prs: &[PullRequest]) -> &'static str {
    let any_merged = prs.iter().any(|p| p.state == "MERGED");
    let any_declined = prs.iter().any(|p| p.state == "DECLINED");
    match (any_merged, any_declined) {
        (true, true) => "Merged / declined",
        (true, false) => "Merged",
        (false, true) => "Declined",
        _ => "Completed",
    }
}

fn print_pr(pr: &PullRequest) {
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

fn render_sprint(s: &SprintStats, stats: StatsMode) {
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

    // Summary mode stops here: sprint name + issue count are structural
    // facts, not personal performance metrics.
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
    print_cycle("Created → Done", s.avg_resolve_hours);
    print_cycle("To Do → Done", s.avg_todo_to_done_hours);
    print_cycle("In Progress", s.avg_in_progress_hours);
    print_cycle("In Review", s.avg_in_review_hours);
    print_cycle("QA", s.avg_qa_hours);
}

fn render_kanban(k: &KanbanStats, stats: StatsMode) {
    println!("Flow (Kanban, last {} days):", k.window_days);
    println!("  WIP: {} open", k.wip_total);
    if stats == StatsMode::Summary {
        // Summary: structural state only — how many open, how many closed
        // in the window. No per-status breakdown, no throughput, no cycle
        // times.
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
    print_cycle("Created → Done", k.avg_resolve_hours);
    print_cycle("To Do → Done", k.avg_todo_to_done_hours);
    print_cycle("In Progress", k.avg_in_progress_hours);
    print_cycle("In Review", k.avg_in_review_hours);
    print_cycle("QA", k.avg_qa_hours);
}

fn print_cycle(label: &str, hours: Option<f64>) {
    let padded = format!("    {:20} ", label);
    match hours {
        Some(h) => println!("{}{}", padded, fmt_duration(h)),
        None => println!("{}—", padded),
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
