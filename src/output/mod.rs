pub mod json;
pub mod markdown;
pub mod text;

use crate::models::StandupData;

pub fn render(data: &StandupData, format: &str) {
    match format {
        "json" => json::render(data),
        "markdown" | "md" => markdown::render(data),
        _ => text::render(data),
    }
}
