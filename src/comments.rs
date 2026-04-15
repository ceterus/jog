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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── adf_to_text ──────────────────────────────────────────────────────

    #[test]
    fn adf_flattens_paragraph_text() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [{"type": "text", "text": "Hello world"}],
            }],
        });
        assert_eq!(adf_to_text(&adf), "Hello world");
    }

    #[test]
    fn adf_joins_multiple_paragraphs_with_newline() {
        let adf = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "line 1"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "line 2"}]},
            ],
        });
        assert_eq!(adf_to_text(&adf), "line 1\nline 2");
    }

    #[test]
    fn adf_skips_code_blocks() {
        let adf = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "before"}]},
                {"type": "codeBlock", "content": [{"type": "text", "text": "SELECT * FROM foo;"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "after"}]},
            ],
        });
        let out = adf_to_text(&adf);
        assert!(out.contains("before"));
        assert!(out.contains("after"));
        assert!(!out.contains("SELECT"));
    }

    #[test]
    fn adf_skips_inline_code() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [
                    {"type": "text", "text": "use "},
                    {"type": "inlineCode", "content": [{"type": "text", "text": "foo()"}]},
                    {"type": "text", "text": " here"},
                ],
            }],
        });
        let out = adf_to_text(&adf);
        assert!(out.contains("use"));
        assert!(out.contains("here"));
        assert!(!out.contains("foo()"));
    }

    #[test]
    fn adf_empty_doc_returns_empty_string() {
        assert_eq!(adf_to_text(&json!({"type": "doc", "content": []})), "");
    }

    // ── clean_comment ────────────────────────────────────────────────────

    #[test]
    fn clean_comment_strips_log_prefix() {
        let out = clean_comment("[2026-04-14] Connected to db\nReal comment here");
        assert_eq!(out, Some("Real comment here".to_string()));
    }

    #[test]
    fn clean_comment_drops_sql_lines() {
        let out =
            clean_comment("SELECT * FROM users;\nINSERT INTO log VALUES (1);\nLooks like a bug");
        assert_eq!(out, Some("Looks like a bug".to_string()));
    }

    #[test]
    fn clean_comment_truncates_over_120_chars() {
        let long = "x".repeat(200);
        let out = clean_comment(&long).unwrap();
        // 120 chars + ellipsis
        assert_eq!(out.chars().count(), 121);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn clean_comment_returns_none_when_all_noise() {
        let out = clean_comment("[log] Connected to postgres\nSELECT 1;\n");
        assert!(out.is_none());
    }

    #[test]
    fn clean_comment_collapses_whitespace_in_first_line() {
        let out = clean_comment("hello   world\tfriend").unwrap();
        assert_eq!(out, "hello world friend");
    }
}
