use crate::comments::clean_comment;
use crate::models::{Flow, KanbanStats, SprintStats, StandupData};
use crate::output::text::fmt_duration;

pub fn render(data: &StandupData) {
    println!("# Standup — {} ({})", data.user_name, data.end_datetime);
    println!();
    println!("## {} ({} → now)", data.since_label, data.start_date);
    if data.activities.is_empty() {
        println!("_No Jira activity found since {}._", data.start_date);
    } else {
        for (key, a) in &data.activities {
            println!("- **[{}]** {} _(status: {})_", key, a.summary, a.status);
            for t in &a.transitions {
                println!("  - transitioned: {}", t);
            }
            if !a.updated_fields.is_empty() {
                let mut f = a.updated_fields.clone();
                f.sort();
                f.dedup();
                println!("  - updated: {}", f.join(", "));
            }
            for c in &a.my_comments {
                if let Some(clean) = clean_comment(c) {
                    println!("  - commented: \"{}\"", clean);
                }
            }
        }
    }
    println!();
    println!("## Today");
    if data.today.is_empty() {
        println!("_No in-progress issues assigned._");
    } else {
        for t in &data.today {
            println!("- **[{}]** {} _({})]_", t.key, t.summary, t.status);
        }
    }
    println!();
    match &data.flow {
        Some(Flow::Sprint(s)) => render_sprint(s),
        Some(Flow::Kanban(k)) => render_kanban(k),
        None => {
            println!("## Sprint");
            println!("_No active sprint found._");
        }
    }
}

fn render_sprint(s: &SprintStats) {
    println!("## Sprint");
    if s.state == "closed" {
        let ended = (-s.days_remaining).max(0);
        let day_word = if ended == 1 { "day" } else { "days" };
        println!(
            "**{}** — closed, ended {} {} ago (was {} days long)",
            s.name, ended, day_word, s.total_days
        );
    } else {
        let day_word = if s.days_remaining == 1 { "day" } else { "days" };
        println!(
            "**{}** — {} {} left of {}",
            s.name, s.days_remaining, day_word, s.total_days
        );
    }
    println!();
    let pct = if s.points_total > 0.0 {
        s.points_done / s.points_total * 100.0
    } else {
        0.0
    };
    println!("| Metric | Value |",);
    println!("| --- | --- |");
    println!(
        "| Points | {}/{} ({:.0}%) |",
        s.points_done, s.points_total, pct
    );
    println!("| Issues | {}/{} |", s.issues_done, s.issues_total);
    if let Some(ppd) = s.points_per_day {
        println!("| Velocity | {:.1} pts/day |", ppd);
        let remaining = s.points_total - s.points_done;
        if s.days_remaining > 0 && remaining > 0.0 {
            println!(
                "| Needed | {:.1} pts/day |",
                remaining / s.days_remaining as f64
            );
        }
    }
    println!();
    println!("**Avg Cycle Times:**");
    println!();
    println!("| Stage | Duration |");
    println!("| --- | --- |");
    print_row("Created → Done", s.avg_resolve_hours);
    print_row("To Do → Done", s.avg_todo_to_done_hours);
    print_row("In Progress", s.avg_in_progress_hours);
    print_row("In Review", s.avg_in_review_hours);
    print_row("QA", s.avg_qa_hours);
}

fn render_kanban(k: &KanbanStats) {
    println!("## Flow (Kanban, last {} days)", k.window_days);
    println!();
    println!("**WIP:** {} open", k.wip_total);
    if !k.wip_by_status.is_empty() {
        println!();
        println!("| Status | Count |");
        println!("| --- | --- |");
        for (status, n) in &k.wip_by_status {
            println!("| {} | {} |", status, n);
        }
    }
    println!();
    println!("**Throughput:** {} issues done", k.throughput);
    if let Some(tpd) = k.throughput_per_day {
        println!();
        println!("{:.2} issues/day", tpd);
    }
    println!();
    println!("**Avg Cycle Times:**");
    println!();
    println!("| Stage | Duration |");
    println!("| --- | --- |");
    print_row("Created → Done", k.avg_resolve_hours);
    print_row("To Do → Done", k.avg_todo_to_done_hours);
    print_row("In Progress", k.avg_in_progress_hours);
    print_row("In Review", k.avg_in_review_hours);
    print_row("QA", k.avg_qa_hours);
}

fn print_row(label: &str, hours: Option<f64>) {
    match hours {
        Some(h) => println!("| {} | {} |", label, fmt_duration(h)),
        None => println!("| {} | — |", label),
    }
}
