use anyhow::{anyhow, Context, Result};
use keyring::Entry;
use std::env;
use std::io::{self, Write};

pub const KEYCHAIN_SERVICE_TOKEN: &str = "jog_api_token";
pub const KEYCHAIN_SERVICE_EMAIL: &str = "jog_email";
pub const KEYCHAIN_SERVICE_URL: &str = "jog_base_url";
pub const KEYCHAIN_SERVICE_BITBUCKET_WORKSPACE: &str = "jog_bitbucket_workspace";
/// Optional: a Bitbucket-scoped Atlassian API token. If absent, Bitbucket
/// calls reuse the main `jog_api_token` (user may have one broadly-scoped
/// token, or separate ones per product).
pub const KEYCHAIN_SERVICE_BITBUCKET_TOKEN: &str = "jog_bitbucket_api_token";
/// Optional: comma-separated list of Bitbucket project keys. When set,
/// the repo listing is filtered to those projects — dramatically
/// cheaper than scanning every repo in a large workspace.
pub const KEYCHAIN_SERVICE_BITBUCKET_PROJECTS: &str = "jog_bitbucket_projects";

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

/// Best-effort delete; silently succeeds if the entry doesn't exist.
pub fn keychain_delete(service: &str) -> Result<()> {
    let entry = Entry::new(service, &whoami()).context("build keyring entry")?;
    let _ = entry.delete_credential();
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

/// Like `prompt` but returns the current value when input is empty and no
/// error when there is no current value. For optional fields (e.g. Bitbucket
/// workspace): user can leave blank to skip, or type `-` to clear a stored
/// value.
pub fn prompt_optional(label: &str, current: Option<&str>) -> Result<Option<String>> {
    let display = current.unwrap_or("(skip)");
    eprint!("{} [{}]: ", label, display);
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();

    if input == "-" {
        return Ok(None); // explicit clear
    }
    if input.is_empty() {
        return Ok(current.map(|s| s.to_string()));
    }
    Ok(Some(input))
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
    let cur_ws = keychain_get(KEYCHAIN_SERVICE_BITBUCKET_WORKSPACE);

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

    println!();
    println!("  Bitbucket PR summary (optional).");
    println!("  Enter your Bitbucket workspace slug (e.g. \"myorg\" from");
    println!("  bitbucket.org/myorg/). Leave blank to skip, or type \"-\" to");
    println!("  clear a stored value.");
    println!();
    println!("  Bitbucket uses the same Atlassian API token as Jira with Basic");
    println!("  auth (email + token). If your main token above isn't scoped for");
    println!("  Bitbucket, create a Bitbucket-scoped token at");
    println!("  https://id.atlassian.com/manage-profile/security/api-tokens");
    println!("  and paste it when prompted. Scopes needed: read:account,");
    println!("  read:pullrequest:bitbucket, read:workspace:bitbucket.");
    println!();
    let cur_bb_token = keychain_get(KEYCHAIN_SERVICE_BITBUCKET_TOKEN);
    let cur_bb_projects = keychain_get(KEYCHAIN_SERVICE_BITBUCKET_PROJECTS);

    let ws = prompt_optional("Bitbucket workspace", cur_ws.as_deref())?;
    let bb_token = prompt_optional(
        "Bitbucket API token (blank to reuse main token)",
        cur_bb_token.as_deref(),
    )?;
    let bb_projects = prompt_optional(
        "Bitbucket project keys (comma-separated, blank = all repos)",
        cur_bb_projects.as_deref(),
    )?;

    let ws_set = !ws.as_deref().unwrap_or("").trim().is_empty();

    if ws_set {
        keychain_set(KEYCHAIN_SERVICE_BITBUCKET_WORKSPACE, ws.as_deref().unwrap())?;
        match bb_token {
            Some(t) if !t.trim().is_empty() => {
                keychain_set(KEYCHAIN_SERVICE_BITBUCKET_TOKEN, &t)?;
            }
            _ => {
                // Explicit clear or declined — main token will be used.
                let _ = keychain_delete(KEYCHAIN_SERVICE_BITBUCKET_TOKEN);
            }
        }
        match bb_projects {
            Some(p) if !p.trim().is_empty() => {
                keychain_set(KEYCHAIN_SERVICE_BITBUCKET_PROJECTS, p.trim())?;
            }
            _ => {
                let _ = keychain_delete(KEYCHAIN_SERVICE_BITBUCKET_PROJECTS);
            }
        }
    } else {
        // No workspace → feature disabled. Clear all BB entries.
        let _ = keychain_delete(KEYCHAIN_SERVICE_BITBUCKET_WORKSPACE);
        let _ = keychain_delete(KEYCHAIN_SERVICE_BITBUCKET_TOKEN);
        let _ = keychain_delete(KEYCHAIN_SERVICE_BITBUCKET_PROJECTS);
    }

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
    let bb_ws = keychain_get(KEYCHAIN_SERVICE_BITBUCKET_WORKSPACE);
    let bb_token = keychain_get(KEYCHAIN_SERVICE_BITBUCKET_TOKEN);
    let bb_projects = keychain_get(KEYCHAIN_SERVICE_BITBUCKET_PROJECTS);
    match bb_ws.as_deref() {
        Some(ws) if !ws.is_empty() => {
            let tok_note = match bb_token.as_deref() {
                Some(t) if !t.is_empty() => format!("separate token **** ({} chars)", t.len()),
                _ => "using main token".to_string(),
            };
            let proj_note = match bb_projects.as_deref() {
                Some(p) if !p.is_empty() => format!(", projects: {}", p),
                _ => ", all projects".to_string(),
            };
            println!("  Bitbucket: {} ({}{})", ws, tok_note, proj_note);
        }
        _ => {
            println!("  Bitbucket: (disabled — run `jog setup` to enable)");
        }
    }

    let cfg_path = crate::config::config_path();
    if cfg_path.exists() {
        println!("  Config:    {}", cfg_path.display());
    } else {
        println!("  Config:    (not created — using defaults)");
    }

    println!("\n  Source: env vars > OS credential store > config.toml");
}
