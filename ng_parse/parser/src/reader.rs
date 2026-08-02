//! Logical-line reader: turns raw deck text into logical lines with provenance.
//!
//! Responsibilities (mirrors the earliest stage of ngspice `inpcom.c`):
//!   * strip full-line comments (`*` as the first non-blank character)
//!   * strip inline `$` comments (HSPICE style)
//!   * join continuation lines (a line whose first non-blank char is `+`)
//!   * drop blank lines
//!   * keep source provenance (file + physical line number) for diagnostics
//!     and for the eventual `struct card.linesource` / `linenum`.
//!
//! SPICE is case-insensitive for keywords; we preserve original case in the text
//! and lower-case only where semantics require it (done by later stages).

use std::borrow::Cow;
use std::sync::Arc;

/// One logical line of a deck: continuations joined, comments stripped.
#[derive(Debug, Clone)]
pub struct LogicalLine {
    /// The joined, comment-stripped text (no trailing newline).
    pub text: String,
    /// Source file this line originated from.
    pub file: Arc<str>,
    /// 1-based physical line number of the first physical line of this logical line.
    pub line_no: usize,
}

/// Return the byte offset at which an inline comment begins, if any.
///
/// Mirrors ngspice `inpcom.c::inp_stripcomments_line` (hs/HSPICE mode, which is
/// what the PDK decks select). `cs` is true inside a `.control ... .endc` section.
///
///  * quoted strings (`"..."` / `'...'`, with `\` escapes) are skipped over, so a
///    `$`/`;` inside them is never a comment (e.g. `echo "v = $&x"`),
///  * `;` and `//` always start a comment,
///  * OUTSIDE `.control`, `$` starts a comment regardless of the preceding
///    character — foundry decks write `...=10u$ comment` with no separator,
///  * INSIDE `.control`, `$` is a comment ONLY when followed by a space, so
///    ngspice variable substitutions (`$&tests`, `$n_test`) survive.
fn inline_comment_start(s: &str, cs: bool) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            // skip the quoted string
            let q = c;
            i += 1;
            while i < b.len() && !(b[i] == q && b[i - 1] != b'\\') {
                i += 1;
            }
            i += 1; // step past the closing quote (or off the end)
            continue;
        }
        if c == b';' {
            return Some(i);
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            return Some(i);
        }
        if c == b'$' {
            if !cs {
                return Some(i);
            }
            if i + 1 < b.len() && b[i + 1] == b' ' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Is this physical line a full-line comment? `*` is the SPICE comment marker;
/// ngspice also converts a leading `#` into one (inpcom.c::inp_stripcomments_line).
fn is_full_comment(s: &str) -> bool {
    matches!(s.trim_start().as_bytes().first(), Some(b'*') | Some(b'#'))
}

/// Lowercased first token, for `.control`/`.endc` tracking.
fn first_kw(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_ascii_lowercase()
}

/// Apply HSPICE/shell-style `\\` end-of-line continuation, which happens *before*
/// `+` stitching. Mirrors ngspice `inpcom.c::chk_for_line_continuation`: a line
/// whose last two non-blank chars are `\\` (and which does not start with `*`/`$`)
/// continues on the next physical line. ngspice implements this by blanking the
/// `\\` and prepending `+` to the following line, turning it into an ordinary
/// continuation — so we do exactly that and let `+` stitching finish the job.
///
/// Line numbering is preserved: each output line still corresponds 1:1 to an input
/// physical line (we never add or remove lines here, only rewrite their content).
fn splice_shell_continuations(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 16);
    let mut prev_continues = false;
    for raw in src.lines() {
        // A line following a `\\` continuation gets a leading `+` (ngspice does
        // this unconditionally, before checking the line itself).
        let line: Cow<str> = if prev_continues {
            Cow::Owned(format!("+{raw}"))
        } else {
            Cow::Borrowed(raw)
        };
        prev_continues = false;

        let trimmed_end = line.trim_end();
        let first = trimmed_end.trim_start().as_bytes().first().copied();
        let emit: &str = if first != Some(b'*')
            && first != Some(b'$')
            && trimmed_end.ends_with("\\\\")
        {
            prev_continues = true;
            &trimmed_end[..trimmed_end.len() - 2] // drop the trailing `\\`
        } else {
            &line
        };
        out.push_str(emit);
        out.push('\n');
    }
    out
}

/// Split raw deck text into logical lines.
///
/// `file` is the provenance label attached to every produced line.
pub fn logical_lines(src: &str, file: Arc<str>) -> Vec<LogicalLine> {
    let mut out: Vec<LogicalLine> = Vec::new();

    let spliced = splice_shell_continuations(src);
    // Comment rules differ inside a `.control ... .endc` section (ngspice passes
    // `found_control` as the `cs` flag to inp_stripcomments_line), so track it.
    let mut in_control = false;
    for (idx, raw) in spliced.lines().enumerate() {
        let line_no = idx + 1;

        // Full-line comments never contribute text and never break continuation
        // joining (a `+` line after a comment still continues the last real line).
        if is_full_comment(raw) {
            continue;
        }

        match first_kw(raw).as_str() {
            ".control" => in_control = true,
            ".endc" => in_control = false,
            _ => {}
        }

        // Strip inline comment (`$`/`;`/`//`), honoring quotes and .control rules.
        let content = match inline_comment_start(raw, in_control) {
            Some(pos) => &raw[..pos],
            None => raw,
        };

        let trimmed = content.trim_end();
        if trimmed.trim_start().is_empty() {
            continue; // blank after stripping
        }

        let lstripped = trimmed.trim_start();
        if let Some(rest) = lstripped.strip_prefix('+') {
            // Continuation: append to the previous logical line, replacing the
            // leading `+` with a single space (ngspice behavior).
            if let Some(last) = out.last_mut() {
                last.text.push(' ');
                last.text.push_str(rest.trim_start());
                continue;
            }
            // No previous line to continue: treat the remainder as its own line.
            out.push(LogicalLine {
                text: rest.trim_start().to_string(),
                file: Arc::clone(&file),
                line_no,
            });
        } else {
            // Emit fully-trimmed: leading whitespace is insignificant in SPICE, and
            // downstream code identifies a card by its FIRST character (device-type
            // letter, `.` for dot-cards). Indented body lines (common in nested
            // subckts) would otherwise present an empty instance name and be
            // misparsed — e.g. `  x31 41a 41b sub3` would not be seen as an X call.
            out.push(LogicalLine {
                text: lstripped.to_string(),
                file: Arc::clone(&file),
                line_no,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ll(src: &str) -> Vec<LogicalLine> {
        logical_lines(src, Arc::from("test"))
    }

    #[test]
    fn joins_continuations() {
        let out = ll("R1 a b 1k\n.param\n+ x = 1\n+ y = 2\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "R1 a b 1k");
        assert_eq!(out[1].text, ".param x = 1 y = 2");
        assert_eq!(out[1].line_no, 2);
    }

    #[test]
    fn strips_full_and_inline_comments() {
        let out = ll("* a comment\nR1 a b 1k $ inline note\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "R1 a b 1k");
        assert_eq!(out[0].line_no, 2);
    }

    #[test]
    fn comment_between_line_and_continuation() {
        // A comment line between a line and its `+` continuation must not break joining.
        let out = ll("V1 n 0 1\n* note\n+ ac 1\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "V1 n 0 1 ac 1");
    }

    #[test]
    fn dollar_comment_hs_mode() {
        // hs/HSPICE mode: `$` ends the line regardless of the preceding char —
        // foundry decks write `l=10u$ comment` with no separator.
        let out = ll("X1 a c $ tail\n");
        assert_eq!(out[0].text, "X1 a c");
        let out = ll("M1 d g s b nch l=10u$ no separator\n");
        assert_eq!(out[0].text, "M1 d g s b nch l=10u");
    }

    #[test]
    fn semicolon_and_slash_comments() {
        assert_eq!(ll("R1 a b 1k ; trailing\n")[0].text, "R1 a b 1k");
        assert_eq!(ll("R1 a b 1k // trailing\n")[0].text, "R1 a b 1k");
    }

    #[test]
    fn hash_line_is_a_comment() {
        assert!(ll("# a comment\nR1 a b 1k\n").len() == 1);
    }

    #[test]
    fn quotes_protect_comment_chars() {
        // a `$` inside a quoted string is not a comment (ngspice skips strings)
        let out = ll("echo \"value = $&x\" $ real comment\n");
        assert_eq!(out[0].text, "echo \"value = $&x\"");
    }

    #[test]
    fn control_block_keeps_dollar_substitutions() {
        // Inside `.control`, `$` is only a comment when followed by a space, so
        // ngspice variable substitutions survive.
        let out = ll(".control\nforeach n $&tests $ note\nset x = $n_test\n.endc\n");
        let texts: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"foreach n $&tests"), "got {texts:?}");
        assert!(texts.contains(&"set x = $n_test"), "got {texts:?}");
    }

    #[test]
    fn skips_blank_lines() {
        let out = ll("\n   \nR1 a b 1k\n\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn joins_backslash_continuation() {
        // HSPICE quoted-expression continuation via trailing `\\`.
        let out = ll(".param f(x)='a*(x==m1)+\\\\\n   b*(x==m2)+\\\\\n   c*(x==m3)'\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, ".param f(x)='a*(x==m1)+ b*(x==m2)+ c*(x==m3)'");
        assert!(!out[0].text.contains('\\'));
    }
}
