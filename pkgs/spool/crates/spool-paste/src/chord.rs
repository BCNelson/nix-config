//! Paste chords and which apps need which.

use std::collections::HashMap;

use crate::keys::KeyCodes;

/// The key combination that makes the target app paste the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PasteChord {
  /// Ctrl+V: ordinary GUI apps.
  CtrlV,
  /// Ctrl+Shift+V: terminal emulators (Ctrl+V is a control character there).
  CtrlShiftV,
  /// Shift+Insert: xterm-style apps (pastes the clipboard under XWayland
  /// xterm only if configured; offered for completeness).
  ShiftInsert,
}

impl PasteChord {
  /// Parses `ctrl+v`, `ctrl+shift+v`, `shift+insert` (case-insensitive), for
  /// config files.
  pub fn parse(s: &str) -> Option<Self> {
    match s.to_ascii_lowercase().replace(' ', "").as_str() {
      "ctrl+v" => Some(Self::CtrlV),
      "ctrl+shift+v" | "shift+ctrl+v" => Some(Self::CtrlShiftV),
      "shift+insert" => Some(Self::ShiftInsert),
      _ => None,
    }
  }

  /// Press/release sequence (evdev keycodes) for this chord: modifiers down,
  /// key down, key up, modifiers up in reverse order.
  pub fn sequence(self, k: &KeyCodes) -> Vec<KeyEvent> {
    let (mods, key): (&[u32], u32) = match self {
      Self::CtrlV => (&[k.ctrl], k.v),
      Self::CtrlShiftV => (&[k.ctrl, k.shift], k.v),
      Self::ShiftInsert => (&[k.shift], k.insert),
    };
    let mut seq = Vec::with_capacity(mods.len() * 2 + 2);
    seq.extend(mods.iter().map(|&code| KeyEvent { code, pressed: true }));
    seq.push(KeyEvent { code: key, pressed: true });
    seq.push(KeyEvent { code: key, pressed: false });
    seq.extend(mods.iter().rev().map(|&code| KeyEvent { code, pressed: false }));
    seq
  }
}

/// One fake key event (evdev keycode, i.e. xkb keycode - 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
  pub code: u32,
  pub pressed: bool,
}

/// App ids that need a chord other than Ctrl+V. Matched case-insensitively,
/// with a trailing `.desktop` ignored (KWin's `desktopFileName` usually has
/// none, but be lenient).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalList {
  map: HashMap<String, PasteChord>,
}

/// Terminal emulators that paste with Ctrl+Shift+V.
pub const DEFAULT_TERMINALS: &[&str] = &[
  "org.kde.konsole",
  "com.mitchellh.ghostty",
  "kitty",
  "foot",
  "footclient",
  "Alacritty",
  "org.wezfurlong.wezterm",
];

fn norm(app_id: &str) -> String {
  let lower = app_id.trim().to_ascii_lowercase();
  lower.strip_suffix(".desktop").map(str::to_owned).unwrap_or(lower)
}

impl Default for TerminalList {
  fn default() -> Self {
    Self::from_terminals(DEFAULT_TERMINALS.iter().copied())
  }
}

impl TerminalList {
  /// An empty list: every app gets Ctrl+V.
  pub fn empty() -> Self {
    Self { map: HashMap::new() }
  }

  /// The given app ids all paste with Ctrl+Shift+V.
  pub fn from_terminals<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
    let mut me = Self::empty();
    for id in ids {
      me.set(id, PasteChord::CtrlShiftV);
    }
    me
  }

  /// Overrides the chord for one app id (e.g. from config).
  pub fn set(&mut self, app_id: &str, chord: PasteChord) {
    self.map.insert(norm(app_id), chord);
  }

  /// Removes an override (the app falls back to Ctrl+V).
  pub fn remove(&mut self, app_id: &str) {
    self.map.remove(&norm(app_id));
  }

  /// Chord to use for `app_id` (`None`/unknown -> Ctrl+V).
  pub fn chord_for(&self, app_id: Option<&str>) -> PasteChord {
    app_id.and_then(|id| self.map.get(&norm(id)).copied()).unwrap_or(PasteChord::CtrlV)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const K: KeyCodes = KeyCodes { ctrl: 29, shift: 42, v: 47, insert: 110 };

  fn ev(code: u32, pressed: bool) -> KeyEvent {
    KeyEvent { code, pressed }
  }

  #[test]
  fn sequences() {
    assert_eq!(
      PasteChord::CtrlV.sequence(&K),
      vec![ev(29, true), ev(47, true), ev(47, false), ev(29, false)]
    );
    assert_eq!(
      PasteChord::CtrlShiftV.sequence(&K),
      vec![ev(29, true), ev(42, true), ev(47, true), ev(47, false), ev(42, false), ev(29, false)]
    );
    assert_eq!(
      PasteChord::ShiftInsert.sequence(&K),
      vec![ev(42, true), ev(110, true), ev(110, false), ev(42, false)]
    );
  }

  #[test]
  fn sequences_are_balanced() {
    for c in [PasteChord::CtrlV, PasteChord::CtrlShiftV, PasteChord::ShiftInsert] {
      let seq = c.sequence(&K);
      let mut down = std::collections::HashSet::new();
      for e in &seq {
        if e.pressed {
          assert!(down.insert(e.code), "{c:?}: double press");
        } else {
          assert!(down.remove(&e.code), "{c:?}: release without press");
        }
      }
      assert!(down.is_empty(), "{c:?}: key left pressed");
    }
  }

  #[test]
  fn terminal_list_defaults() {
    let t = TerminalList::default();
    for id in [
      "org.kde.konsole",
      "com.mitchellh.ghostty",
      "kitty",
      "foot",
      "Alacritty",
      "alacritty",
      "org.wezfurlong.wezterm",
    ] {
      assert_eq!(t.chord_for(Some(id)), PasteChord::CtrlShiftV, "{id}");
    }
    assert_eq!(t.chord_for(Some("org.kde.konsole.desktop")), PasteChord::CtrlShiftV);
    assert_eq!(t.chord_for(Some("org.kde.kate")), PasteChord::CtrlV);
    assert_eq!(t.chord_for(Some("")), PasteChord::CtrlV);
    assert_eq!(t.chord_for(None), PasteChord::CtrlV);
  }

  #[test]
  fn terminal_list_configurable() {
    let mut t = TerminalList::from_terminals(["kitty"]);
    assert_eq!(t.chord_for(Some("foot")), PasteChord::CtrlV);
    t.set("XTerm", PasteChord::ShiftInsert);
    assert_eq!(t.chord_for(Some("xterm")), PasteChord::ShiftInsert);
    t.remove("KITTY");
    assert_eq!(t.chord_for(Some("kitty")), PasteChord::CtrlV);
    assert_eq!(TerminalList::empty().chord_for(Some("org.kde.konsole")), PasteChord::CtrlV);
  }

  #[test]
  fn parse() {
    assert_eq!(PasteChord::parse("Ctrl+V"), Some(PasteChord::CtrlV));
    assert_eq!(PasteChord::parse("ctrl + shift + v"), Some(PasteChord::CtrlShiftV));
    assert_eq!(PasteChord::parse("Shift+Insert"), Some(PasteChord::ShiftInsert));
    assert_eq!(PasteChord::parse("alt+v"), None);
  }
}
