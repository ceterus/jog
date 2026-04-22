//! Layout primitives for the landscape standup card.
//!
//! Two jobs:
//!   1. Measure/truncate/pad strings in *display columns* (not bytes, not
//!      char count) so unicode icons, CJK and emoji don't throw off the
//!      grid.
//!   2. Zip parallel column buffers into rendered rows separated by a
//!      vertical rule.
//!
//! Everything here is ANSI-escape aware — color codes are zero-width and
//! must not contribute to measured width.

use unicode_width::UnicodeWidthStr;

/// Strip ANSI SGR escapes (`ESC[...m`) from `s` in-place for width
/// measurement. Non-allocating check first to avoid the common case.
fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut in_esc = false;
    for c in s.chars() {
        if in_esc {
            if c == 'm' {
                in_esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_esc = true;
            continue;
        }
        out.push(c);
    }
    out
}

/// Display width of `s` ignoring ANSI escapes.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(strip_ansi(s).as_str())
}

/// Pad `s` on the right with spaces until its display width equals
/// `width`. If `s` is already wider, it's truncated with an ellipsis.
pub fn pad_right(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w == width {
        s.to_string()
    } else if w < width {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(width - w));
        out
    } else {
        truncate(s, width)
    }
}

/// Truncate `s` to fit in `max` display columns, appending an ellipsis
/// when anything was dropped. Preserves ANSI escapes passed through.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(s) <= max {
        return s.to_string();
    }
    // Walk characters keeping ANSI bytes free, stop when we'd exceed max - 1
    // to leave room for the ellipsis.
    let mut out = String::with_capacity(s.len());
    let mut w = 0usize;
    let mut in_esc = false;
    let budget = max.saturating_sub(1);
    for c in s.chars() {
        if in_esc {
            out.push(c);
            if c == 'm' {
                in_esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            out.push(c);
            in_esc = true;
            continue;
        }
        let cw = UnicodeWidthStr::width(c.to_string().as_str());
        if w + cw > budget {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    pad_right(&out, max).trim_end().to_string()
}

/// Wrap `text` into lines whose display width is ≤ `width`. Simple greedy
/// word wrap — good enough for ticket summaries and PR titles. Blank
/// inputs produce an empty Vec (not a single empty line).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() || width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    for word in text.split_whitespace() {
        let ww = display_width(word);
        if ww > width {
            // Word alone is longer than the column — hard-truncate it.
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_w = 0;
            }
            lines.push(truncate(word, width));
            continue;
        }
        let need = if current.is_empty() { ww } else { ww + 1 };
        if current_w + need > width {
            lines.push(std::mem::take(&mut current));
            current_w = 0;
        }
        if !current.is_empty() {
            current.push(' ');
            current_w += 1;
        }
        current.push_str(word);
        current_w += ww;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Zip parallel column buffers into rendered rows. Each column is padded
/// to its declared width, then joined with ` │ ` (unicode) or `  |  ` (ascii).
/// Shorter columns are filled with blanks so all rows land at the same
/// total width.
pub fn zip_columns(cols: &[Vec<String>], widths: &[usize], unicode: bool) -> Vec<String> {
    let rows = cols.iter().map(|c| c.len()).max().unwrap_or(0);
    let sep = if unicode { " │ " } else { " | " };
    let mut out = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut parts: Vec<String> = Vec::with_capacity(cols.len());
        for (i, col) in cols.iter().enumerate() {
            let blank = String::new();
            let cell = col.get(r).unwrap_or(&blank);
            parts.push(pad_right(cell, widths[i]));
        }
        out.push(parts.join(sep));
    }
    out
}

/// Horizontal line `n` wide. Unicode `─` or ASCII `-`.
pub fn hline(n: usize, unicode: bool) -> String {
    let ch = if unicode { "─" } else { "-" };
    ch.repeat(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_right_basic() {
        assert_eq!(pad_right("hi", 5), "hi   ");
        assert_eq!(pad_right("hello", 5), "hello");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[test]
    fn truncate_preserves_ansi() {
        let s = "\x1b[36mPROJ-412\x1b[0m long summary here";
        let t = truncate(s, 12);
        // ANSI escapes shouldn't count toward the width.
        assert!(display_width(&t) <= 12);
    }

    #[test]
    fn wrap_basic() {
        let v = wrap("hello world from jog", 11);
        assert_eq!(v, vec!["hello world".to_string(), "from jog".to_string()]);
    }

    #[test]
    fn zip_pads_short_columns() {
        let a = vec!["a".into(), "aa".into(), "aaa".into()];
        let b = vec!["b".into()];
        let out = zip_columns(&[a, b], &[5, 3], true);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], "a     │ b  ");
        assert_eq!(out[1], "aa    │    ");
    }
}
