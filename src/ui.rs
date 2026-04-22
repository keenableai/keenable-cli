//! Shared terminal UI helpers for consistent CLI output styling.
//!
//! Design: completed steps render as dimmed+strikethrough, the final
//! result stands out in color, and 👀 marks command headers.
//!
//! All output is word-wrapped so continuation lines align with the text
//! start position, keeping paragraphs clean at any terminal width.

use colored::{ColoredString, Colorize};

const EYES: &str = "👀";

/// Text starts at column 5 for top-level icon items: "   ✓ "
const TOP_CONT_WIDTH: usize = 5;
/// Text starts at column 4 for plain items: "   " + 1 char
const PLAIN_CONT_WIDTH: usize = 4;
/// Text starts at column 11 for sub-items: "      - ⚠ "
const SUB_CONT_WIDTH: usize = 11;

// ── Terminal width ─────────────────────────────────────────────────

fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
}

// ── Word wrapping ──────────────────────────────────────────────────

/// Word-wrap `text` to fit within `width` columns.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Print wrapped text with a displayed prefix and aligned continuation.
/// `cont_width` is the display width where text starts (used for continuation indent).
fn print_wrapped(
    prefix: impl std::fmt::Display,
    cont_width: usize,
    msg: &str,
    style: impl Fn(&str) -> ColoredString,
) {
    // +2 margin accounts for emoji icons that may render as 2 display columns,
    // ensuring our word-wrap fires before the terminal's native line break.
    let text_width = terminal_width().saturating_sub(cont_width + 2);
    let lines = word_wrap(msg, text_width);
    let cont = " ".repeat(cont_width);

    if let Some((first, rest)) = lines.split_first() {
        eprintln!("{}{}", prefix, style(first));
        for line in rest {
            eprintln!("{}{}", cont, style(line));
        }
    }
}

// ── Cursor helpers ─────────────────────────────────────────────────

/// Save the current cursor position (DEC private mode).
pub fn save_cursor() {
    eprint!("\x1b7");
}

/// Restore the saved cursor position and clear everything below it.
pub fn restore_and_clear() {
    eprint!("\x1b8\x1b[J");
}

// ── Top-level steps ─────────────────────────────────────────────────

/// Print a branded command header: `👀 message`
pub fn header(msg: &str) {
    eprintln!("\n{}  {}\n", EYES, msg.bold());
}

/// Print a completed intermediate step (dimmed + strikethrough).
pub fn step_done(msg: &str) {
    print_wrapped(
        format!("   {} ", "✓".dimmed()),
        TOP_CONT_WIDTH,
        msg,
        |s| s.dimmed().strikethrough(),
    );
}

/// Print an in-progress step (shows a spinner-like marker).
/// Saves cursor position so `step_done_replace` can overwrite it.
pub fn step(msg: &str) {
    save_cursor();
    print_wrapped(
        format!("   {} ", "…".dimmed()),
        TOP_CONT_WIDTH,
        msg,
        |s| s.dimmed(),
    );
}

/// Replace the last `step()` output with a completed step (dimmed + strikethrough).
pub fn step_done_replace(msg: &str) {
    restore_and_clear();
    step_done(msg);
}

/// Print the final success line (green, bold).
pub fn success(msg: &str) {
    print_wrapped(
        format!("   {} ", "✓".green().bold()),
        TOP_CONT_WIDTH,
        msg,
        |s| s.green(),
    );
}

/// Print a failure line (red).
pub fn error(msg: &str) {
    print_wrapped(
        format!("   {} ", "✗".red().bold()),
        TOP_CONT_WIDTH,
        msg,
        |s| s.red(),
    );
}

/// Print a warning line (yellow).
pub fn warning(msg: &str) {
    print_wrapped(
        format!("   {} ", "⚠".yellow()),
        TOP_CONT_WIDTH,
        msg,
        |s| s.yellow(),
    );
}

/// Print a hint / next-step line.
pub fn hint(msg: &str) {
    print_wrapped(
        format!("\n{}", " ".repeat(PLAIN_CONT_WIDTH)),
        PLAIN_CONT_WIDTH,
        msg,
        |s| s.dimmed(),
    );
}

/// Print an indented info line.
pub fn info(msg: &str) {
    print_wrapped(
        " ".repeat(PLAIN_CONT_WIDTH),
        PLAIN_CONT_WIDTH,
        msg,
        |s| s.normal(),
    );
}

// ── Sub-steps (one extra indent level) ──────────────────────────────

/// Print a completed sub-step (dimmed + strikethrough, extra indent).
pub fn sub_done(msg: &str) {
    print_wrapped(
        format!("      - {} ", "✓".dimmed()),
        SUB_CONT_WIDTH,
        msg,
        |s| s.dimmed().strikethrough(),
    );
}

/// Print a sub-step success (green, extra indent).
pub fn sub_success(msg: &str) {
    print_wrapped(
        format!("      - {} ", "✓".green()),
        SUB_CONT_WIDTH,
        msg,
        |s| s.green(),
    );
}

/// Print a sub-step warning (yellow, extra indent).
pub fn sub_warning(msg: &str) {
    print_wrapped(
        format!("      - {} ", "⚠".yellow()),
        SUB_CONT_WIDTH,
        msg,
        |s| s.yellow(),
    );
}

/// Print a sub-step info line (extra indent).
pub fn sub_info(msg: &str) {
    print_wrapped(
        "      - ",
        SUB_CONT_WIDTH,
        msg,
        |s| s.normal(),
    );
}

/// Print a dimmed sub-step hint with warning icon (extra indent).
pub fn sub_hint(msg: &str) {
    print_wrapped(
        format!("      - {} ", "⚠".yellow()),
        SUB_CONT_WIDTH,
        msg,
        |s| s.dimmed(),
    );
}

// ── Section labels ──────────────────────────────────────────────────

/// Print a section label (bold, same indent as steps).
pub fn label(msg: &str) {
    eprintln!("\n   {}", msg.bold());
}
