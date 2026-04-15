use crate::models::StandupData;

pub fn render(data: &StandupData) {
    match serde_json::to_string_pretty(data) {
        Ok(json) => println!("{}", json),
        Err(e) => eprintln!("JSON serialization error: {}", e),
    }
}
