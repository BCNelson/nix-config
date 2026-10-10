//! "Edit in external editor" keys: Ctrl+E edits the selected item's
//! preferred representation, Ctrl+Shift+E opens a small chooser of its
//! formats first. The picker only names the item and the format
//! (`PickerReq::Edit`); spoold reads the content and starts the editor.
//! Pure logic (tested here); `app.rs` wires it to the UI.

use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};
use spool_proto::WireItemId;

/// The text variants, in preference order (mirrors
/// `spool_core::item::TEXT_MIMES`; the picker depends on spool-proto only).
/// They are one format to the user: spoold edits any of them as UTF-8 text.
const TEXT_MIMES: &[&str] =
  &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];

fn is_text_variant(m: &str) -> bool {
  TEXT_MIMES.iter().any(|t| t.eq_ignore_ascii_case(m))
}

/// The formats a user can edit, one per kind of content: the best text
/// variant first (if any), then every other mime in the item's order.
pub fn editable_formats(mimes: &[String]) -> Vec<String> {
  let mut out = Vec::new();
  if let Some(t) = TEXT_MIMES.iter().find(|t| mimes.iter().any(|m| m.eq_ignore_ascii_case(t))) {
    out.push((*t).to_string());
  }
  for m in mimes {
    if !is_text_variant(m) && !out.iter().any(|o| o.eq_ignore_ascii_case(m)) {
      out.push(m.clone());
    }
  }
  out
}

/// What Ctrl+E edits: text first (like paste), else the first format.
pub fn preferred_format(mimes: &[String]) -> Option<String> {
  editable_formats(mimes).into_iter().next()
}

/// A short label for the chooser.
pub fn format_label(mime: &str) -> String {
  let ess = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
  let what = match ess.as_str() {
    _ if is_text_variant(mime) => "Plain text",
    "text/html" => "HTML",
    "text/uri-list" => "Links",
    "image/png" => "PNG image",
    "image/jpeg" => "JPEG image",
    "image/webp" => "WebP image",
    _ => return crate::sanitize::sanitize_line(mime, 60),
  };
  format!("{what} · {}", crate::sanitize::sanitize_line(mime, 60))
}

/// An edit shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
  /// Ctrl+E.
  Preferred,
  /// Ctrl+Shift+E.
  Choose,
}

/// Ctrl+E / Ctrl+Shift+E (not on key repeat, not with Alt/Super).
pub fn edit_key(k: Keysym, m: &Modifiers, repeat: bool) -> Option<EditKey> {
  if repeat || !m.ctrl || m.alt || m.logo || !matches!(k, Keysym::e | Keysym::E) {
    return None;
  }
  Some(if m.shift { EditKey::Choose } else { EditKey::Preferred })
}

/// The format chooser (modal over the list while open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chooser {
  pub id: WireItemId,
  pub formats: Vec<String>,
  pub current: usize,
}

/// What a key did in the chooser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChooserAction {
  /// Consumed (moved, or ignored).
  Stay,
  /// Edit this format.
  Pick(String),
  /// Closed without a choice (back to the list).
  Close,
}

impl Chooser {
  /// `None` when the item has nothing to edit.
  pub fn new(id: WireItemId, mimes: &[String]) -> Option<Self> {
    let formats = editable_formats(mimes);
    (!formats.is_empty()).then_some(Self { id, formats, current: 0 })
  }

  /// Up/Down/Home/End move, Enter or 1-9 pick, Esc closes; every other key
  /// is swallowed (the search box must not receive it).
  pub fn on_key(&mut self, k: Keysym) -> ChooserAction {
    let last = self.formats.len().saturating_sub(1);
    match k {
      Keysym::Escape => return ChooserAction::Close,
      Keysym::Up | Keysym::KP_Up => self.current = self.current.saturating_sub(1),
      Keysym::Down | Keysym::KP_Down => self.current = (self.current + 1).min(last),
      Keysym::Home | Keysym::KP_Home => self.current = 0,
      Keysym::End | Keysym::KP_End => self.current = last,
      Keysym::Return | Keysym::KP_Enter => {
        return ChooserAction::Pick(self.formats[self.current].clone());
      }
      _ => {
        let digit = k.key_char().and_then(|c| c.to_digit(10)).filter(|d| *d >= 1);
        if let Some(d) = digit
          && let Some(f) = self.formats.get(d as usize - 1)
        {
          return ChooserAction::Pick(f.clone());
        }
      }
    }
    ChooserAction::Stay
  }

  /// Labels for the UI, numbered.
  pub fn labels(&self) -> Vec<String> {
    self
      .formats
      .iter()
      .enumerate()
      .map(|(i, m)| if i < 9 { format!("{}  {}", i + 1, format_label(m)) } else { format_label(m) })
      .collect()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn v(m: &[&str]) -> Vec<String> {
    m.iter().map(|s| s.to_string()).collect()
  }

  #[test]
  fn formats_collapse_text_variants() {
    let firefox =
      v(&["text/html", "text/plain;charset=utf-8", "UTF8_STRING", "TEXT", "STRING", "text/plain"]);
    assert_eq!(editable_formats(&firefox), ["text/plain;charset=utf-8", "text/html"]);
    assert_eq!(preferred_format(&firefox).as_deref(), Some("text/plain;charset=utf-8"));
    assert_eq!(preferred_format(&v(&["UTF8_STRING", "TEXT"])).as_deref(), Some("UTF8_STRING"));
    let img = v(&["image/png", "image/webp"]);
    assert_eq!(editable_formats(&img), ["image/png", "image/webp"]);
    assert_eq!(preferred_format(&img).as_deref(), Some("image/png"));
    assert_eq!(preferred_format(&[]), None);
  }

  #[test]
  fn edit_keys() {
    let m = |ctrl, shift| Modifiers { ctrl, shift, ..Default::default() };
    assert_eq!(edit_key(Keysym::e, &m(true, false), false), Some(EditKey::Preferred));
    assert_eq!(edit_key(Keysym::E, &m(true, true), false), Some(EditKey::Choose));
    assert_eq!(edit_key(Keysym::e, &m(true, false), true), None, "not on repeat");
    assert_eq!(edit_key(Keysym::e, &m(false, false), false), None, "plain e is typing");
    assert_eq!(edit_key(Keysym::t, &m(true, false), false), None);
    let alt = Modifiers { ctrl: true, alt: true, ..Default::default() };
    assert_eq!(edit_key(Keysym::e, &alt, false), None);
  }

  #[test]
  fn chooser_navigation_and_picks() {
    let mimes = v(&["text/html", "text/plain", "image/png"]);
    let mut c = Chooser::new(7, &mimes).unwrap();
    assert_eq!(c.formats, ["text/plain", "text/html", "image/png"]);
    assert_eq!(c.labels()[0], "1  Plain text · text/plain");
    assert_eq!(c.labels()[2], "3  PNG image · image/png");
    assert_eq!(c.on_key(Keysym::Up), ChooserAction::Stay);
    assert_eq!(c.current, 0);
    c.on_key(Keysym::Down);
    c.on_key(Keysym::Down);
    c.on_key(Keysym::Down);
    assert_eq!(c.current, 2, "clamped");
    c.on_key(Keysym::Home);
    c.on_key(Keysym::Down);
    assert_eq!(c.on_key(Keysym::Return), ChooserAction::Pick("text/html".into()));
    assert_eq!(c.on_key(Keysym::_3), ChooserAction::Pick("image/png".into()));
    assert_eq!(c.on_key(Keysym::_4), ChooserAction::Stay, "no 4th format");
    assert_eq!(c.on_key(Keysym::_0), ChooserAction::Stay);
    assert_eq!(c.on_key(Keysym::x), ChooserAction::Stay, "swallowed");
    assert_eq!(c.on_key(Keysym::Escape), ChooserAction::Close);
    assert!(Chooser::new(1, &[]).is_none());
    assert_eq!(format_label("application/x-thing"), "application/x-thing");
  }
}
