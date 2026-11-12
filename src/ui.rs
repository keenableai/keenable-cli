//! Shared terminal UI helpers for consistent CLI output styling.
//!
//! Design: completed steps render as dimmed+strikethrough, the final
//! result stands out in color, and 👀 marks command headers.
//!
//! All output is word-wrapped so continuation lines align with the text
//! start position, keeping paragraphs clean at any terminal width.

use colored::{ColoredString, Colorize};
use std::io::IsTerminal;

const EYES: &str = "👀";

// ── Terminal width ─────────────────────────────────────────────────

fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
}

// ── Word wrapping ──────────────────────────────────────────────────

/// Display width of `s`, skipping ANSI escape sequences — messages often
/// embed colored spans (e.g. `"keenable login".cyan()`) whose escape bytes
/// must not count as columns or lines wrap far short of the terminal edge.
fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            len += 1;
        }
    }
    len
}

/// Word-wrap `text` to fit within `width` display columns.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    for word in text.split_whitespace() {
        let word_len = visible_len(word);
        if current.is_empty() {
            current = word.to_string();
            current_len = word_len;
        } else if current_len + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
            current_len += 1 + word_len;
        } else {
            lines.push(current);
            current = word.to_string();
            current_len = word_len;
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

/// Print wrapped text with a displayed prefix; continuation lines are
/// indented to align with where the first line's text starts.
/// Returns the number of terminal lines printed.
fn print_wrapped(
    prefix: impl std::fmt::Display,
    msg: &str,
    style: impl Fn(&str) -> ColoredString,
) -> usize {
    let prefix = prefix.to_string();
    // prefix may start with a leading newline (ui::hint) — measure the last line
    let cont_width = visible_len(prefix.rsplit('\n').next().unwrap_or(""));
    // +2 margin accounts for emoji icons that may render as 2 display columns,
    // ensuring our word-wrap fires before the terminal's native line break.
    let text_width = terminal_width().saturating_sub(cont_width + 2);
    let lines = word_wrap(msg, text_width);
    let cont = " ".repeat(cont_width);

    let mut printed = prefix.matches('\n').count();
    if let Some((first, rest)) = lines.split_first() {
        eprintln!("{}{}", prefix, style(first));
        printed += 1;
        for line in rest {
            eprintln!("{}{}", cont, style(line));
            printed += 1;
        }
    }
    printed
}

// ── Cursor helpers ─────────────────────────────────────────────────

/// Save the current cursor position (DEC private mode). No-op when stderr
/// is not a terminal — the escapes would just litter piped output.
pub fn save_cursor() {
    if std::io::stderr().is_terminal() {
        eprint!("\x1b7");
    }
}

/// Restore the saved cursor position and clear everything below it.
pub fn restore_and_clear() {
    if std::io::stderr().is_terminal() {
        eprint!("\x1b8\x1b[J");
    }
}

// ── Top-level steps ─────────────────────────────────────────────────

/// Print a branded command header: `👀 message`
pub fn header(msg: &str) {
    eprintln!("\n{}  {}\n", EYES, msg.bold());
}

/// Print a completed intermediate step (dimmed + strikethrough).
pub fn step_done(msg: &str) {
    print_wrapped(format!("   {}  ", "✓".dimmed()), msg, |s| {
        s.dimmed().strikethrough()
    });
}

/// Print an in-progress step (shows a spinner-like marker).
/// Returns the number of lines printed, for `step_done_replace`.
pub fn step(msg: &str) -> usize {
    print_wrapped(format!("   {}  ", "…".dimmed()), msg, |s| s.dimmed())
}

/// Replace the last `step()` output (which printed `lines` lines) with a
/// completed step. Moves the cursor up relatively — unlike DEC save/restore,
/// this stays correct if the terminal scrolled during a long wait.
pub fn step_done_replace(msg: &str, lines: usize) {
    if std::io::stderr().is_terminal() && lines > 0 {
        eprint!("\x1b[{}A\x1b[J", lines);
    }
    step_done(msg);
}

/// Print the final success line (green, bold).
pub fn success(msg: &str) {
    print_wrapped(format!("   {}  ", "✓".green().bold()), msg, |s| s.green());
}

/// Print a failure line (red).
pub fn error(msg: &str) {
    print_wrapped(format!("   {}  ", "✗".red().bold()), msg, |s| s.red());
}

/// `msg` on one line: control characters (line breaks, escape sequences the
/// API text might carry) become spaces and runs of whitespace collapse, so
/// the result never spans lines or restyles the terminal.
pub fn one_line(msg: &str) -> String {
    msg.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text from the API or a web page, safe to print in pretty mode: control
/// characters (escape sequences that restyle the terminal, rewrite its title
/// or write the clipboard) and bidirectional overrides (which reorder what
/// is shown) are dropped. Line breaks and tabs stay.
pub fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| match c {
            '\n' | '\t' => true,
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => false,
            c => !c.is_control(),
        })
        .collect()
}

/// Write machine output to stdout. A reader that went away (`| head`, a
/// closed pipe) is not an error: the rest is dropped and the command ends
/// with its own exit code, instead of Rust's broken-pipe panic (exit 101).
pub fn stdout(text: &str) {
    write_or_drop(&mut std::io::stdout().lock(), text);
}

fn write_or_drop(out: &mut impl std::io::Write, text: &str) {
    let _ = out.write_all(text.as_bytes()).and_then(|_| out.flush());
}

/// Print `error: <msg>` to stderr as exactly one unstyled line, never
/// wrapped: the summary YAML mode adds for scripts that read stderr.
pub fn error_line(msg: &str) {
    eprintln!("error: {}", one_line(msg));
}

/// Print a warning line (yellow).
pub fn warning(msg: &str) {
    print_wrapped(format!("   {}  ", "⚠".yellow()), msg, |s| s.yellow());
}

/// Print a hint / next-step line.
pub fn hint(msg: &str) {
    print_wrapped("\n    ", msg, |s| s.dimmed());
}

/// Print an indented info line.
pub fn info(msg: &str) {
    print_wrapped("    ", msg, |s| s.normal());
}

// ── Sub-steps (one extra indent level) ──────────────────────────────

/// Print a completed sub-step (dimmed + strikethrough, extra indent).
pub fn sub_done(msg: &str) {
    print_wrapped(format!("      - {}  ", "✓".dimmed()), msg, |s| {
        s.dimmed().strikethrough()
    });
}

/// Print a sub-step success (green, extra indent).
pub fn sub_success(msg: &str) {
    print_wrapped(format!("      - {}  ", "✓".green()), msg, |s| s.green());
}

/// Print a sub-step error (red, extra indent).
pub fn sub_error(msg: &str) {
    print_wrapped(format!("      - {}  ", "✗".red().bold()), msg, |s| {
        s.red()
    });
}

/// Print a sub-step warning (yellow, extra indent).
pub fn sub_warning(msg: &str) {
    print_wrapped(format!("      - {}  ", "⚠".yellow()), msg, |s| s.yellow());
}

/// Print a sub-step info line (extra indent).
pub fn sub_info(msg: &str) {
    print_wrapped("      - ", msg, |s| s.normal());
}

/// Print a dimmed sub-step hint with warning icon (extra indent).
pub fn sub_hint(msg: &str) {
    print_wrapped(format!("      - {}  ", "⚠".yellow()), msg, |s| s.dimmed());
}

// ── Section labels ──────────────────────────────────────────────────

/// Print a section label (bold, same indent as steps).
pub fn label(msg: &str) {
    eprintln!("\n   {}", msg.bold());
}

#[cfg(test)]
mod one_line_tests {
    use super::one_line;

    #[test]
    fn one_line_collapses_line_breaks_however_long_the_message() {
        let msg = format!("first line\nsecond\r\n\tline {}", "x".repeat(500));
        let line = one_line(&msg);
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
        assert!(line.starts_with("first line second line xxx"));
    }

    #[test]
    fn writing_to_a_closed_reader_does_not_panic() {
        struct Closed;
        impl std::io::Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        super::write_or_drop(&mut Closed, "results: []\n");
        let mut buf = Vec::new();
        super::write_or_drop(&mut buf, "ok\n");
        assert_eq!(buf, b"ok\n");
    }

    #[test]
    fn printable_drops_escapes_and_bidi_overrides_but_keeps_layout() {
        let page = "Title\u{1b}]0;pwned\u{7}\n\tline \u{9b}2J\u{202e}txt.exe\u{2066}";
        assert_eq!(super::printable(page), "Title]0;pwned\n\tline 2Jtxt.exe");
        assert_eq!(super::printable("plain ünïcode 👀"), "plain ünïcode 👀");
    }

    #[test]
    fn one_line_drops_terminal_escape_sequences() {
        let line = one_line("bad \u{1b}[31mred\u{1b}[0m \u{7}bell \u{9b}2J");
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert_eq!(line, "bad [31mred [0m bell 2J");
    }
}
