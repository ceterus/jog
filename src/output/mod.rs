pub mod json;
pub mod markdown;
pub mod text;

use crate::config::StatsMode;
use crate::models::StandupData;

pub fn render(data: &StandupData, format: &str, stats: StatsMode) {
    match format {
        "json" => json::render(data),
        "markdown" | "md" => markdown::render(data, stats),
        _ => text::render(data, stats),
    }
}
