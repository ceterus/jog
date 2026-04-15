use anyhow::Result;
use reqwest::Client;
use serde_json::Value;

use crate::client::get_json;
use crate::config::Credentials;

pub async fn comments_on(
    client: &Client,
    creds: &Credentials,
    issue_key: &str,
) -> Result<Vec<Value>> {
    let path = format!("/rest/api/3/issue/{}/comment?maxResults=100", issue_key);
    let v = get_json(client, creds, &path).await?;
    Ok(v.get("comments")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default())
}

pub fn adf_to_text(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                // Skip code blocks entirely (inline and fenced)
                if let Some(Value::String(ty)) = m.get("type") {
                    if ty == "codeBlock" || ty == "inlineCode" {
                        return;
                    }
                }
                if let Some(Value::String(t)) = m.get("text") {
                    out.push_str(t);
                }
                if let Some(Value::Array(c)) = m.get("content") {
                    for child in c {
                        walk(child, out);
                    }
                    if let Some(Value::String(ty)) = m.get("type") {
                        if matches!(
                            ty.as_str(),
                            "paragraph" | "heading" | "listItem" | "bulletList" | "orderedList"
                        ) {
                            out.push('\n');
                        }
                    }
                }
            }
            Value::Array(a) => {
                for x in a {
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    let mut s = String::new();
    walk(v, &mut s);
    s.trim().to_string()
}

pub fn clean_comment(s: &str) -> Option<String> {
    let lines: Vec<&str> = s.lines().collect();
    let meaningful: Vec<&str> = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with('[')
                && !t.to_uppercase().starts_with("SET ")
                && !t.to_uppercase().starts_with("SELECT")
                && !t.to_uppercase().starts_with("INSERT")
                && !t.to_uppercase().starts_with("UPDATE ")
                && !t.to_uppercase().starts_with("DELETE ")
                && !t.to_uppercase().starts_with("ALTER ")
                && !t.to_uppercase().starts_with("CREATE ")
                && !t.to_uppercase().starts_with("DROP ")
                && !t.to_uppercase().starts_with("CONNECT")
                && !t.to_uppercase().starts_with("EXEC")
                && !t.contains("completed in")
                && !t.contains("search_path")
                && !looks_like_log(t)
        })
        .copied()
        .collect();
    if meaningful.is_empty() {
        return None;
    }
    let first = meaningful[0];
    let flat: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    let max = 120;
    if flat.chars().count() <= max {
        Some(flat)
    } else {
        let truncated: String = flat.chars().take(max).collect();
        Some(format!("{}…", truncated))
    }
}

fn looks_like_log(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.len() < 5 {
        return false;
    }
    trimmed.contains("] Connected")
        || trimmed.contains("] playmaker")
        || trimmed.contains("public>")
        || (trimmed.starts_with("20") && trimmed.chars().nth(4) == Some('-'))
}
