//! Make untrusted clipboard text safe to hand to Slint as *plain* text.
//!
//! Control characters, bidi overrides/isolates/marks, zero-width and other
//! invisible formatting characters are replaced by visible markers, so a
//! preview can never reorder itself (an RLO in `invoice<RLO>fdp.exe`), hide content, or
//! smuggle invisible text past the user. The output is a single line.

/// Maximum characters in a sanitized preview line (before the ellipsis).
pub const PREVIEW_CHARS: usize = 240;

/// Marker for a character that must not be rendered as-is, or `None` if the
/// character is safe.
fn marker(c: char) -> Option<std::borrow::Cow<'static, str>> {
  use std::borrow::Cow::{Borrowed, Owned};
  let cp = c as u32;
  Some(match cp {
    // C0 controls -> Unicode "control pictures" (␀ .. ␟); \n -> ␊, \t -> ␉.
    0x00..=0x1F => {
      let pic = char::from_u32(0x2400 + cp).unwrap_or('\u{FFFD}');
      return Some(Owned(pic.to_string()));
    }
    0x7F => Borrowed("␡"),
    // C1 controls (NEL etc.).
    0x80..=0x9F => return Some(Owned(format!("⟪U+{cp:04X}⟫"))),
    0x00AD => Borrowed("⟪SHY⟫"),
    0x034F => Borrowed("⟪CGJ⟫"),
    0x061C => Borrowed("⟪ALM⟫"),
    0x115F | 0x1160 | 0x3164 | 0xFFA0 => Borrowed("⟪HF⟫"), // Hangul fillers
    0x180E => Borrowed("⟪MVS⟫"),
    0x200B => Borrowed("⟪ZWSP⟫"),
    0x200C => Borrowed("⟪ZWNJ⟫"),
    0x200D => Borrowed("⟪ZWJ⟫"),
    0x200E => Borrowed("⟪LRM⟫"),
    0x200F => Borrowed("⟪RLM⟫"),
    0x2028 => Borrowed("⟪LS⟫"),
    0x2029 => Borrowed("⟪PS⟫"),
    0x202A => Borrowed("⟪LRE⟫"),
    0x202B => Borrowed("⟪RLE⟫"),
    0x202C => Borrowed("⟪PDF⟫"),
    0x202D => Borrowed("⟪LRO⟫"),
    0x202E => Borrowed("⟪RLO⟫"),
    0x2060 => Borrowed("⟪WJ⟫"),
    0x2061..=0x2064 => return Some(Owned(format!("⟪U+{cp:04X}⟫"))),
    0x2066 => Borrowed("⟪LRI⟫"),
    0x2067 => Borrowed("⟪RLI⟫"),
    0x2068 => Borrowed("⟪FSI⟫"),
    0x2069 => Borrowed("⟪PDI⟫"),
    0x206A..=0x206F => return Some(Owned(format!("⟪U+{cp:04X}⟫"))),
    0xFEFF => Borrowed("⟪BOM⟫"),
    0xFFF9..=0xFFFB => return Some(Owned(format!("⟪U+{cp:04X}⟫"))), // interlinear annotation
    // Tag characters: invisible, used for "ASCII smuggling".
    0xE0000..=0xE007F => return Some(Owned(format!("⟪TAG U+{cp:05X}⟫"))),
    _ => return None,
  })
}

/// Sanitize `input` into one visible line of at most `max_chars` characters
/// (markers count by their own length); appends `…` when truncated.
pub fn sanitize_line(input: &str, max_chars: usize) -> String {
  let mut out = String::with_capacity(input.len().min(max_chars.saturating_mul(4)) + 4);
  let mut count = 0usize;
  for c in input.chars() {
    let piece_len;
    match marker(c) {
      Some(m) => {
        piece_len = m.chars().count();
        if count.saturating_add(piece_len) > max_chars {
          out.push('…');
          return out;
        }
        out.push_str(&m);
      }
      None => {
        piece_len = 1;
        if count.saturating_add(1) > max_chars {
          out.push('…');
          return out;
        }
        out.push(c);
      }
    }
    count += piece_len;
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn plain_text_unchanged() {
    assert_eq!(sanitize_line("hello, wörld — ok", 100), "hello, wörld — ok");
    // Genuine RTL letters are fine; only the formatting controls are not.
    assert_eq!(sanitize_line("שלום", 100), "שלום");
  }

  #[test]
  fn controls_become_pictures() {
    assert_eq!(sanitize_line("a\nb\tc\r\0", 100), "a␊b␉c␍␀");
    assert_eq!(sanitize_line("\u{7f}\u{1b}[31m", 100), "␡␛[31m");
    assert_eq!(sanitize_line("x\u{85}y", 100), "x⟪U+0085⟫y");
  }

  #[test]
  fn bidi_overrides_are_neutralized() {
    let s = sanitize_line("invoice\u{202E}fdp.exe", 100);
    assert_eq!(s, "invoice⟪RLO⟫fdp.exe");
    for (c, m) in [
      ('\u{202A}', "⟪LRE⟫"),
      ('\u{202B}', "⟪RLE⟫"),
      ('\u{202C}', "⟪PDF⟫"),
      ('\u{202D}', "⟪LRO⟫"),
      ('\u{2066}', "⟪LRI⟫"),
      ('\u{2067}', "⟪RLI⟫"),
      ('\u{2068}', "⟪FSI⟫"),
      ('\u{2069}', "⟪PDI⟫"),
      ('\u{200E}', "⟪LRM⟫"),
      ('\u{200F}', "⟪RLM⟫"),
      ('\u{061C}', "⟪ALM⟫"),
    ] {
      assert_eq!(sanitize_line(&c.to_string(), 100), m);
    }
  }

  #[test]
  fn invisible_chars_are_visible() {
    assert_eq!(sanitize_line("pa\u{200B}ss", 100), "pa⟪ZWSP⟫ss");
    assert_eq!(sanitize_line("\u{FEFF}x\u{2060}", 100), "⟪BOM⟫x⟪WJ⟫");
    assert_eq!(sanitize_line("a\u{200C}\u{200D}", 100), "a⟪ZWNJ⟫⟪ZWJ⟫");
    assert_eq!(sanitize_line("\u{E0041}", 100), "⟪TAG U+E0041⟫");
    assert_eq!(sanitize_line("l1\u{2028}l2", 100), "l1⟪LS⟫l2");
  }

  #[test]
  fn output_has_no_unsafe_chars() {
    let nasty: String = (0u32..0x3000)
      .chain(0xE0000..0xE0080)
      .chain([0xFEFF, 0xFFF9, 0xFFFA, 0xFFFB])
      .filter_map(char::from_u32)
      .collect();
    let out = sanitize_line(&nasty, usize::MAX);
    for c in out.chars() {
      let cp = c as u32;
      assert!(cp >= 0x20 && cp != 0x7F, "control {cp:#x} leaked");
      assert!(!(0x80..=0x9F).contains(&cp), "C1 {cp:#x} leaked");
      assert!(!(0x202A..=0x202E).contains(&cp), "bidi {cp:#x} leaked");
      assert!(!(0x2066..=0x2069).contains(&cp), "isolate {cp:#x} leaked");
      assert!(!(0x200B..=0x200F).contains(&cp), "zero-width {cp:#x} leaked");
      assert!(!(0xE0000..=0xE007F).contains(&cp), "tag {cp:#x} leaked");
      assert!(cp != 0xFEFF && cp != 0x2060 && cp != 0x2028 && cp != 0x2029);
    }
  }

  #[test]
  fn truncates_on_char_boundaries() {
    assert_eq!(sanitize_line("abcdef", 3), "abc…");
    assert_eq!(sanitize_line("ééééé", 2), "éé…");
    // A marker that would not fit is dropped whole, never split.
    assert_eq!(sanitize_line("ab\u{202E}", 4), "ab…");
    assert_eq!(sanitize_line("", 3), "");
    let long = "x".repeat(10_000);
    assert_eq!(sanitize_line(&long, PREVIEW_CHARS).chars().count(), PREVIEW_CHARS + 1);
  }
}
