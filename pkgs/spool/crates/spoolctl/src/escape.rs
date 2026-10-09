//! Terminal-safe rendering of clipboard text.

use std::fmt::Write;

/// Escape characters that can manipulate a terminal or hide content when
/// printing to a TTY: C0/C1 controls except `\n` and `\t` (as `\xNN` /
/// `\u{NN}`), ESC, DEL, bidi controls (U+202A..U+202E, U+2066..U+2069,
/// U+200E/U+200F, U+061C) and zero-width/invisible characters (U+200B..U+200D,
/// U+2060, U+FEFF) as `\u{XXXX}`. Invalid UTF-8 is rendered lossily with
/// U+FFFD.
pub fn escape_for_tty(data: &[u8]) -> String {
  let text = String::from_utf8_lossy(data);
  let mut out = String::with_capacity(text.len());
  for c in text.chars() {
    match c {
      '\n' | '\t' => out.push(c),
      '\0'..='\x1f' | '\x7f' => {
        let _ = write!(out, "\\x{:02x}", c as u32);
      }
      '\u{80}'..='\u{9f}' => {
        let _ = write!(out, "\\u{{{:02x}}}", c as u32);
      }
      c if is_invisible_or_bidi(c) => {
        let _ = write!(out, "\\u{{{:04X}}}", c as u32);
      }
      c => out.push(c),
    }
  }
  out
}

fn is_invisible_or_bidi(c: char) -> bool {
  matches!(
    c,
    '\u{202A}'..='\u{202E}'
      | '\u{2066}'..='\u{2069}'
      | '\u{200E}'
      | '\u{200F}'
      | '\u{061C}'
      | '\u{200B}'..='\u{200D}'
      | '\u{2060}'
      | '\u{FEFF}'
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn plain_text_unchanged() {
    assert_eq!(escape_for_tty(b"hello\n\tworld"), "hello\n\tworld");
    assert_eq!(escape_for_tty("héllo ✓".as_bytes()), "héllo ✓");
  }

  #[test]
  fn controls_escaped() {
    assert_eq!(escape_for_tty(b"\x1b[31mred\x1b[0m"), "\\x1b[31mred\\x1b[0m");
    assert_eq!(escape_for_tty(b"a\rb\x00c\x7f"), "a\\x0db\\x00c\\x7f");
    assert_eq!(escape_for_tty("x\u{9b}y".as_bytes()), "x\\u{9b}y");
  }

  #[test]
  fn bidi_and_zero_width_escaped() {
    assert_eq!(escape_for_tty("a\u{202E}b".as_bytes()), "a\\u{202E}b");
    assert_eq!(escape_for_tty("\u{2066}x\u{2069}".as_bytes()), "\\u{2066}x\\u{2069}");
    assert_eq!(escape_for_tty("a\u{200B}b\u{FEFF}".as_bytes()), "a\\u{200B}b\\u{FEFF}");
    assert_eq!(
      escape_for_tty("\u{200E}\u{061C}\u{2060}".as_bytes()),
      "\\u{200E}\\u{061C}\\u{2060}"
    );
  }

  #[test]
  fn invalid_utf8_lossy() {
    assert_eq!(escape_for_tty(b"a\xffb"), "a\u{FFFD}b");
  }
}
