use anyhow::{anyhow, Context, Result};
use keyring::Entry;
use std::env;
use std::io::{self, Write};

pub const KEYCHAIN_SERVICE_TOKEN: &str = "jog_api_token";
pub const KEYCHAIN_SERVICE_EMAIL: &str = "jog_email";
pub const KEYCHAIN_SERVICE_URL: &str = "jog_base_url";

/// Cross-platform credential lookup. Uses the OS-native credential store via
/// the `keyring` crate: macOS Keychain, Linux Secret Service, Windows
/// Credential Manager.
pub fn keychain_get(service: &str) -> Option<String> {
    let entry = Entry::new(service, &whoami()).ok()?;
    let val = entry.get_password().ok()?.trim().to_string();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

pub fn keychain_set(service: &str, value: &str) -> Result<()> {
    let entry = Entry::new(service, &whoami()).context("build keyring entry")?;
    // keyring's set_password overwrites on all backends; no manual delete needed.
    entry
        .set_password(value)
        .map_err(|e| anyhow!("credential store write failed: {}", e))?;
    Ok(())
}

/// Username used as the "account" field in the credential store. Preserves the
/// pre-keyring macOS layout (USER env var) so existing entries still resolve,
/// and falls back to USERNAME for Windows.
fn whoami() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "jog".to_string())
}

pub fn prompt(label: &str, current: Option<&str>, secret: bool) -> Result<String> {
    if let Some(c) = current {
        let display = if secret {
            let n = c.len();
            if n > 8 {
                format!("{}...{}", &c[..4], &c[n - 4..])
            } else {
                "****".to_string()
            }
        } else {
            c.to_string()
        };
        eprint!("{} [{}]: ", label, display);
    } else {
        eprint!("{}: ", label);
    }
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();

    if input.is_empty() {
        current
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("{} is required", label))
    } else {
        Ok(input)
    }
}

pub fn run_setup() -> Result<()> {
    println!("jog setup — credentials stored in your OS credential store\n");

    println!(
        "  You need an Atlassian API token (non-scoped — works for Jira, Bitbucket, Confluence)."
    );
    println!("  1. Open: https://id.atlassian.com/manage-profile/security/api-tokens");
    println!("  2. Click \"Create API token\" (NOT \"Create API token with scopes\")");
    println!("  3. Name it anything (e.g. jog), copy the token");
    println!();

    let cur_url = keychain_get(KEYCHAIN_SERVICE_URL);
    let cur_email = keychain_get(KEYCHAIN_SERVICE_EMAIL);
    let cur_token = keychain_get(KEYCHAIN_SERVICE_TOKEN);

    let url = prompt(
        "Jira Base URL (e.g. https://myorg.atlassian.net)",
        cur_url.as_deref(),
        false,
    )?;
    let url = url.trim_end_matches('/').to_string();
    let email = prompt("Jira Email", cur_email.as_deref(), false)?;
    let token = prompt("API Token", cur_token.as_deref(), true)?;

    keychain_set(KEYCHAIN_SERVICE_URL, &url)?;
    keychain_set(KEYCHAIN_SERVICE_EMAIL, &email)?;
    keychain_set(KEYCHAIN_SERVICE_TOKEN, &token)?;

    println!("\n✓ Credentials saved to your OS credential store.");
    println!("  Run `jog` to see your standup.");
    Ok(())
}

pub fn run_config() {
    println!("jog config:\n");
    let url = keychain_get(KEYCHAIN_SERVICE_URL);
    let email = keychain_get(KEYCHAIN_SERVICE_EMAIL);
    let token = keychain_get(KEYCHAIN_SERVICE_TOKEN);

    println!("  Base URL:  {}", url.as_deref().unwrap_or("(not set)"));
    println!("  Email:     {}", email.as_deref().unwrap_or("(not set)"));
    match token {
        Some(t) => {
            let n = t.len();
            if n > 8 {
                println!("  API Token: {}...{} ({} chars)", &t[..4], &t[n - 4..], n);
            } else {
                println!("  API Token: **** ({} chars)", n);
            }
        }
        None => println!("  API Token: (not set)"),
    }

    let cfg_path = crate::config::config_path();
    if cfg_path.exists() {
        println!("  Config:    {}", cfg_path.display());
    } else {
        println!("  Config:    (not created — using defaults)");
    }

    println!("\n  Source: env vars > OS credential store > config.toml");
}
