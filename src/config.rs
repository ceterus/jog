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

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct JiraConfig {
    pub base_url: String,
    pub projects: Vec<String>,
    pub board_id: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct FieldsConfig {
    pub story_points: String,
    pub sprint: String,
}

impl Default for FieldsConfig {
    fn default() -> Self {
        Self {
            story_points: "customfield_10047".to_string(),
            sprint: "customfield_10010".to_string(),
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
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format: "text".to_string(),
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
