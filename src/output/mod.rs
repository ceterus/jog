pub mod json;
pub mod layout;
pub mod markdown;
pub mod text;
pub mod theme;

use crate::config::{LayoutMode, StatsMode};
use crate::models::StandupData;

pub fn render(data: &StandupData, format: &str, stats: StatsMode, layout: LayoutMode) {
    match format {
        "json" => json::render(data),
        "markdown" | "md" => markdown::render(data, stats),
        _ => text::render(data, stats, layout),
    }
}
