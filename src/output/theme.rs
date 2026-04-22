//! Terminal capability detection (width, color, unicode) for the text
//! renderer. Detection runs once per render; the resulting `Theme` is
//! threaded through the layout code so individual formatters don't have
//! to re-probe the environment.

use std::env;
use std::io::IsTerminal;

/// Hard upper bound — wider terminals waste readability on long lines.
const MAX_WIDTH: usize = 140;
/// Target design width for the landscape layout.
pub const TARGET_WIDTH: usize = 120;
/// Below this we fall back to the stacked (single-column TUI) layout.
/// The card styling (boxed header, colour, icons, bars) is preserved —
/// sections just stack vertically instead of sitting side-by-side.
pub const STACKED_CUTOFF: usize = 80;

#[derive(Clone, Debug)]
pub struct Theme {
    /// Effective render width in columns.
    pub width: usize,
    /// Whether ANSI color codes should be emitted.
    pub color: bool,
    /// Whether the terminal can render non-ASCII box / bar / icon glyphs.
    pub unicode: bool,
}

impl Theme {
    pub fn detect() -> Self {
        let color = detect_color();
        let unicode = detect_unicode();
        let width = detect_width();
        Theme {
            width,
            color,
            unicode,
        }
    }

    #[cfg(test)]
    pub fn plain(width: usize) -> Self {
        Theme {
            width,
            color: false,
            unicode: true,
        }
    }

    #[cfg(test)]
    pub fn plain_landscape() -> Self {
        Self::plain(TARGET_WIDTH)
    }
}

fn detect_width() -> usize {
    // Honour an explicit override first — useful for testing and for users
    // piping to pagers.
    if let Ok(s) = env::var("JOG_WIDTH") {
        if let Ok(n) = s.parse::<usize>() {
            return n.clamp(40, MAX_WIDTH);
        }
    }
    if !std::io::stdout().is_terminal() {
        // Non-TTY (pipe, file): render at target width so the output reads
        // the same when captured. Don't stretch to whatever COLUMNS says.
        return TARGET_WIDTH;
    }
    match terminal_size::terminal_size() {
        Some((terminal_size::Width(w), _)) => (w as usize).clamp(40, MAX_WIDTH),
        None => TARGET_WIDTH,
    }
}

fn detect_color() -> bool {
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
        return false;
    }
    if env::var_os("JOG_NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

fn detect_unicode() -> bool {
    if env::var_os("JOG_ASCII").is_some() {
        return false;
    }
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = env::var(key) {
            let up = v.to_ascii_uppercase();
            if up.contains("UTF-8") || up.contains("UTF8") {
                return true;
            }
            if !v.is_empty() && up != "C" && up != "POSIX" {
                // Non-empty, non-C locale without explicit UTF-8 — assume it.
                return true;
            }
        }
    }
    // No locale hints at all → assume modern terminal.
    true
}

// --- ANSI wrappers. All no-op when `theme.color` is false. ---

const RESET: &str = "\x1b[0m";

pub fn cyan(s: &str, theme: &Theme) -> String {
    ansi(s, theme, "\x1b[36m")
}
pub fn yellow(s: &str, theme: &Theme) -> String {
    ansi(s, theme, "\x1b[33m")
}
pub fn green(s: &str, theme: &Theme) -> String {
    ansi(s, theme, "\x1b[32m")
}
pub fn red(s: &str, theme: &Theme) -> String {
    ansi(s, theme, "\x1b[31m")
}
pub fn dim(s: &str, theme: &Theme) -> String {
    ansi(s, theme, "\x1b[2m")
}
pub fn bold(s: &str, theme: &Theme) -> String {
    ansi(s, theme, "\x1b[1m")
}

fn ansi(s: &str, theme: &Theme, code: &str) -> String {
    if theme.color {
        format!("{}{}{}", code, s, RESET)
    } else {
        s.to_string()
    }
}
