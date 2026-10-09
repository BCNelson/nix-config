//! Keysym -> evdev keycode resolution under the current xkb keymap.

use xkbcommon::xkb;

/// evdev codes from `linux/input-event-codes.h`, used when the keymap has no
/// key for a keysym (or no keymap arrived).
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_V: u32 = 47;
pub const KEY_INSERT: u32 = 110;

/// xkb keycodes are evdev codes + 8.
const EVDEV_OFFSET: u32 = 8;

/// evdev keycodes for the keys a paste chord needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCodes {
  pub ctrl: u32,
  pub shift: u32,
  pub v: u32,
  pub insert: u32,
}

impl KeyCodes {
  /// Plain US-QWERTY evdev codes (the fallback without a keymap).
  pub const FALLBACK: KeyCodes =
    KeyCodes { ctrl: KEY_LEFTCTRL, shift: KEY_LEFTSHIFT, v: KEY_V, insert: KEY_INSERT };

  /// Resolves the chord keys in `keymap`.
  ///
  /// `layout` is the currently active layout (group) index if known (KWin:
  /// `org.kde.keyboard /Layouts getLayout`). Layouts are searched starting
  /// with that one, then in order 0..n, so for a non-Latin active layout the
  /// first Latin layout's `v` key is used (Qt/GTK match Ctrl+<key> shortcuts
  /// against the Latin layout the same way). Only shift level 1 counts.
  pub fn resolve(keymap: &xkb::Keymap, layout: Option<u32>) -> KeyCodes {
    let find = |sym: u32, fallback: u32| {
      find_keycode(keymap, xkb::Keysym::new(sym), layout).unwrap_or(fallback)
    };
    KeyCodes {
      ctrl: find(xkb::keysyms::KEY_Control_L, KEY_LEFTCTRL),
      shift: find(xkb::keysyms::KEY_Shift_L, KEY_LEFTSHIFT),
      v: find(xkb::keysyms::KEY_v, KEY_V),
      insert: find(xkb::keysyms::KEY_Insert, KEY_INSERT),
    }
  }
}

/// Lowest evdev keycode whose level-1 symbol in some layout is exactly `sym`;
/// the preferred `layout` is searched first.
pub fn find_keycode(keymap: &xkb::Keymap, sym: xkb::Keysym, layout: Option<u32>) -> Option<u32> {
  let n = keymap.num_layouts();
  let mut order: Vec<u32> = Vec::with_capacity(n as usize + 1);
  if let Some(l) = layout.filter(|&l| l < n) {
    order.push(l);
  }
  order.extend((0..n).filter(|&l| Some(l) != layout));
  let min = keymap.min_keycode().raw().max(EVDEV_OFFSET);
  let max = keymap.max_keycode().raw();
  for l in order {
    for kc in min..=max {
      let key = xkb::Keycode::new(kc);
      // Keys with fewer layouts than `l` would wrap; skip them so a key is
      // only matched under the layout it actually belongs to.
      if keymap.num_layouts_for_key(key) <= l {
        continue;
      }
      if keymap.key_get_syms_by_level(key, l, 0) == [sym] {
        return Some(kc - EVDEV_OFFSET);
      }
    }
  }
  None
}

/// Compiles a keymap from RMLVO names (tests and fallbacks). Environment
/// defaults (`XKB_DEFAULT_*`) are not consulted for options.
pub fn keymap_from_names(layout: &str, variant: &str, options: &str) -> Option<xkb::Keymap> {
  let ctx = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
  xkb::Keymap::new_from_names(
    &ctx,
    "evdev",
    "pc105",
    layout,
    variant,
    Some(options.to_owned()),
    xkb::COMPILE_NO_FLAGS,
  )
}

/// libxkbcommon's default keymap, honouring `XKB_DEFAULT_RULES/MODEL/
/// LAYOUT/VARIANT/OPTIONS` (US QWERTY otherwise). Used for the virtual
/// keyboard when the seat has no keyboard to copy a keymap from.
pub fn default_keymap() -> Option<xkb::Keymap> {
  let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
  xkb::Keymap::new_from_names(&ctx, "", "", "", "", None, xkb::COMPILE_NO_FLAGS)
}

/// Compiles the text keymap a compositor sent (`wl_keyboard.keymap`,
/// `xkb_v1` format). Trailing NULs are stripped.
pub fn keymap_from_string(text: String) -> Option<xkb::Keymap> {
  let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
  let text = match text.find('\0') {
    Some(i) => text[..i].to_owned(),
    None => text,
  };
  xkb::Keymap::new_from_string(&ctx, text, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::COMPILE_NO_FLAGS)
}

#[cfg(test)]
mod tests {
  use super::*;

  // evdev codes for the expectations below.
  const KEY_DOT: u32 = 52;
  const KEY_CAPSLOCK: u32 = 58;

  fn km(layout: &str, variant: &str, options: &str) -> xkb::Keymap {
    keymap_from_names(layout, variant, options).unwrap_or_else(|| {
      panic!("compile keymap {layout}({variant}) [{options}] — is xkeyboard-config available?")
    })
  }

  #[test]
  fn us_qwerty() {
    let k = KeyCodes::resolve(&km("us", "", ""), None);
    assert_eq!(k, KeyCodes::FALLBACK);
  }

  #[test]
  fn dvorak_moves_v() {
    let k = KeyCodes::resolve(&km("us", "dvorak", ""), None);
    assert_eq!(k.v, KEY_DOT, "Dvorak 'v' sits on the QWERTY '.' key");
    assert_eq!((k.ctrl, k.shift, k.insert), (KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_INSERT));
  }

  #[test]
  fn azerty_keeps_v() {
    let map = km("fr", "", "");
    let k = KeyCodes::resolve(&map, None);
    assert_eq!(k.v, KEY_V);
    // Sanity: the keymap really is AZERTY ('a' on the QWERTY 'q' key, 16).
    assert_eq!(find_keycode(&map, xkb::Keysym::new(xkb::keysyms::KEY_a), None), Some(16));
  }

  #[test]
  fn non_latin_first_layout_uses_latin_layout() {
    let k = KeyCodes::resolve(&km("ru,us", "", ""), Some(0));
    assert_eq!(k.v, KEY_V);
  }

  #[test]
  fn active_layout_preferred() {
    let map = km("us,us", "dvorak,", "");
    assert_eq!(KeyCodes::resolve(&map, None).v, KEY_DOT);
    assert_eq!(KeyCodes::resolve(&map, Some(0)).v, KEY_DOT);
    assert_eq!(KeyCodes::resolve(&map, Some(1)).v, KEY_V);
    // Out-of-range index is ignored.
    assert_eq!(KeyCodes::resolve(&map, Some(7)).v, KEY_DOT);
  }

  #[test]
  fn swapped_ctrl_caps() {
    let k = KeyCodes::resolve(&km("us", "", "ctrl:swapcaps"), None);
    assert_eq!(k.ctrl, KEY_CAPSLOCK, "Control_L is produced by the Caps key");
  }

  #[test]
  fn keymap_text_roundtrip_with_nul() {
    let map = km("us", "dvorak", "");
    let mut text = map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    text.push('\0');
    let back = keymap_from_string(text).expect("recompile");
    assert_eq!(KeyCodes::resolve(&back, None).v, KEY_DOT);
  }
}
