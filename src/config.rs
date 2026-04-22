use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::auth;

/// Top-level config file structure (~/.config/jog/config.toml)
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    pub jira: JiraConfig,
    pub fields: FieldsConfig,
    pub statuses: StatusesConfig,
    pub ai: AiConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct JiraConfig {
    pub base_url: String,
    pub projects: Vec<String>,
    pub board_id: Option<u64>,
    /// Flow mode: "auto" (detect), "scrum" (force sprint queries),
    /// "kanban" (skip sprint queries entirely).
    pub mode: String,
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            projects: vec![],
            board_id: None,
            mode: "auto".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct FieldsConfig {
    pub story_points: String,
    pub sprint: String,
    /// Field names to hide from the "updated fields" row in ticket cards.
    /// Matched case-insensitively against Jira's `field` value (or its
    /// rendered alias). `Rank` is excluded by default because it carries
    /// an opaque LexoRank string with no human meaning.
    pub exclude: Vec<String>,
}

impl Default for FieldsConfig {
    fn default() -> Self {
        Self {
            story_points: "customfield_10047".to_string(),
            sprint: "customfield_10010".to_string(),
            exclude: vec!["Rank".to_string()],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct StatusesConfig {
    pub in_progress: Vec<String>,
    pub in_review: Vec<String>,
    pub qa: Vec<String>,
    pub done: Vec<String>,
}

impl Default for StatusesConfig {
    fn default() -> Self {
        Self {
            in_progress: vec!["In Progress".to_string()],
            in_review: vec!["IN REVIEW".to_string()],
            qa: vec!["QA".to_string()],
            done: vec![
                "Done".to_string(),
                "Closed".to_string(),
                "Resolved".to_string(),
            ],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct AiConfig {
    pub provider: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "none".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct OutputConfig {
    pub format: String,
    /// How much of the stats panel to render:
    /// - "full" (default): points, velocity, throughput, cycle times
    /// - "summary": just structural facts (sprint name, days left, N done / M total)
    /// - "off": hide the stats panel entirely
    pub stats: String,
    /// Text-renderer layout:
    /// - "card" (default): auto-adaptive — landscape tri-column card on
    ///   wide terminals, stacked single-column card on narrow ones.
    /// - "stacked": force the stacked card layout regardless of terminal
    ///   width. Same colour/icons/bars as the landscape card, sections
    ///   just flow top-to-bottom.
    /// - "plain": legacy single-column text, no box-drawing, no colour.
    ///   Use when piping into other tools or sharing to quieter channels.
    pub layout: String,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format: "text".to_string(),
            stats: "full".to_string(),
            layout: "card".to_string(),
        }
    }
}

/// Parsed stats-visibility level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsMode {
    Full,
    Summary,
    Off,
}

impl StatsMode {
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "off" | "none" | "hide" | "false" => Self::Off,
            "summary" | "brief" | "terse" => Self::Summary,
            _ => Self::Full,
        }
    }
}

/// Text-renderer layout. Only affects `--format text` (JSON and markdown
/// outputs are layout-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Auto — landscape tri-column at ≥80 cols, stacked below. Default.
    Card,
    /// Force the stacked single-column card (same styling as landscape,
    /// sections stacked vertically). Useful when you like the narrow look
    /// on a wide terminal.
    Stacked,
    /// Legacy single-column plain text (no box-drawing, no colour).
    Plain,
}

impl LayoutMode {
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "plain" | "simple" | "legacy" | "classic" | "text" => Self::Plain,
            "stacked" | "vertical" | "column" | "narrow" => Self::Stacked,
            _ => Self::Card,
        }
    }
}

/// Runtime credentials resolved from env vars → keychain → config file
pub struct Credentials {
    pub base_url: String,
    pub email: String,
    pub token: String,
}

impl Credentials {
    pub fn resolve(app_config: &AppConfig) -> Result<Self> {
        let token = env::var("JIRA_API_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| auth::keychain_get(auth::KEYCHAIN_SERVICE_TOKEN))
            .context("JIRA_API_TOKEN not set and not in Keychain. Run `jog setup`.")?
            .trim()
            .to_string();

        let email = env::var("JIRA_EMAIL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| auth::keychain_get(auth::KEYCHAIN_SERVICE_EMAIL))
            .context("JIRA_EMAIL not set and not in Keychain. Run `jog setup`.")?
            .trim()
            .to_string();

        let base_url = env::var("JIRA_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| auth::keychain_get(auth::KEYCHAIN_SERVICE_URL))
            .or_else(|| {
                let u = &app_config.jira.base_url;
                if u.is_empty() {
                    None
                } else {
                    Some(u.clone())
                }
            })
            .context("JIRA_BASE_URL not set and not in Keychain. Run `jog setup`.")?
            .trim()
            .trim_end_matches('/')
            .to_string();

        Ok(Self {
            base_url,
            email,
            token,
        })
    }

    pub fn auth_header(&self) -> String {
        use base64::{engine::general_purpose, Engine as _};
        let raw = format!("{}:{}", self.email, self.token);
        format!("Basic {}", general_purpose::STANDARD.encode(raw))
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("jog")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    eprintln!("[warn] config parse error: {}. Using defaults.", e);
                }
            },
            Err(e) => {
                eprintln!("[warn] config read error: {}. Using defaults.", e);
            }
        }
    }
    AppConfig::default()
}

pub fn project_jql_clause(projects: &[String]) -> String {
    match projects.len() {
        0 => String::new(),
        1 => format!("project = {}", projects[0]),
        _ => format!("project IN ({})", projects.join(", ")),
    }
}

pub fn done_statuses_jql(statuses: &[String]) -> String {
    let quoted: Vec<String> = statuses.iter().map(|s| format!("\"{}\"", s)).collect();
    quoted.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn project_clause_empty() {
        assert_eq!(project_jql_clause(&[]), "");
    }

    #[test]
    fn project_clause_single() {
        assert_eq!(project_jql_clause(&s(&["PROJ"])), "project = PROJ");
    }

    #[test]
    fn project_clause_multiple() {
        assert_eq!(
            project_jql_clause(&s(&["PROJ", "INFRA"])),
            "project IN (PROJ, INFRA)"
        );
    }

    #[test]
    fn done_statuses_quotes_and_joins() {
        assert_eq!(
            done_statuses_jql(&s(&["Done", "Closed", "Resolved"])),
            "\"Done\", \"Closed\", \"Resolved\""
        );
    }

    #[test]
    fn done_statuses_empty() {
        assert_eq!(done_statuses_jql(&[]), "");
    }

    #[test]
    fn default_app_config_has_expected_field_ids() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.fields.story_points, "customfield_10047");
        assert_eq!(cfg.fields.sprint, "customfield_10010");
        assert_eq!(cfg.output.format, "text");
        assert_eq!(cfg.ai.provider, "none");
        assert!(cfg.jira.base_url.is_empty());
        assert!(cfg.jira.projects.is_empty());
    }

    #[test]
    fn default_statuses_include_core_set() {
        let cfg = AppConfig::default();
        assert!(cfg.statuses.in_progress.iter().any(|s| s == "In Progress"));
        assert!(cfg.statuses.done.iter().any(|s| s == "Done"));
        assert!(cfg.statuses.done.iter().any(|s| s == "Closed"));
        assert!(cfg.statuses.done.iter().any(|s| s == "Resolved"));
    }

    // ── StatsMode ────────────────────────────────────────────────────────

    #[test]
    fn stats_mode_full_default() {
        assert_eq!(AppConfig::default().output.stats, "full");
        assert_eq!(StatsMode::from_str("full"), StatsMode::Full);
        // Unknown strings fall back to Full — same safe default behaviour
        // as FlowMode.
        assert_eq!(StatsMode::from_str("bananas"), StatsMode::Full);
        assert_eq!(StatsMode::from_str(""), StatsMode::Full);
    }

    #[test]
    fn stats_mode_parses_off_aliases() {
        for s in ["off", "none", "hide", "false", "OFF", " off "] {
            assert_eq!(StatsMode::from_str(s), StatsMode::Off, "failed on {s:?}");
        }
    }

    #[test]
    fn stats_mode_parses_summary_aliases() {
        for s in ["summary", "brief", "terse", "SUMMARY"] {
            assert_eq!(
                StatsMode::from_str(s),
                StatsMode::Summary,
                "failed on {s:?}"
            );
        }
    }
}
