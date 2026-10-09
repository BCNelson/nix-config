//! Pure parsers for Hyprland IPC replies and events (unit-tested against
//! fixtures in `tests/fixtures`).

use serde::Deserialize;

use crate::HyprError;

/// The focused client as far as Spool cares: no title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprWindow {
  /// `0x`-prefixed lowercase hex address (the stable window id).
  pub address: String,
  /// Wayland app id / X11 class; `None` when empty.
  pub class: Option<String>,
}

/// One line from `.socket2.sock` that Spool reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyprEvent {
  /// `activewindow>>CLASS,TITLE` (`>>,` when nothing is focused). Only the
  /// class is kept.
  ActiveWindow { class: Option<String> },
  /// `activewindowv2>>ADDRESS` (empty or `,` when nothing is focused).
  ActiveWindowV2 { address: Option<String> },
  /// `closewindow>>ADDRESS`.
  CloseWindow { address: Option<String> },
}

/// `0x55d5c0a3b2c0` / `55d5c0a3b2c0` -> `0x55d5c0a3b2c0`; `None` for
/// anything that is not 1..=16 hex digits.
pub fn normalize_address(raw: &str) -> Option<String> {
  let raw = raw.trim();
  let hex = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")).unwrap_or(raw);
  if hex.is_empty() || hex.len() > 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
    return None;
  }
  Some(format!("0x{}", hex.to_ascii_lowercase()))
}

fn nonempty(s: &str) -> Option<String> {
  let s = s.trim();
  (!s.is_empty()).then(|| s.to_owned())
}

/// Parses one event line (without the trailing newline). Unknown events and
/// malformed lines are `None`.
pub fn parse_event(line: &str) -> Option<HyprEvent> {
  let (name, data) = line.split_once(">>")?;
  match name {
    // The class cannot contain the separator in practice; the title can, so
    // split at the first comma.
    "activewindow" => {
      Some(HyprEvent::ActiveWindow { class: nonempty(data.split(',').next().unwrap_or("")) })
    }
    "activewindowv2" => {
      let d = data.trim().trim_end_matches(',');
      Some(HyprEvent::ActiveWindowV2 { address: normalize_address(d) })
    }
    "closewindow" => Some(HyprEvent::CloseWindow { address: normalize_address(data) }),
    _ => None,
  }
}

#[derive(Deserialize)]
struct CursorJson {
  x: f64,
  y: f64,
}

/// `j/cursorpos` reply.
pub fn parse_cursorpos(json: &str) -> Result<(i32, i32), HyprError> {
  let c: CursorJson =
    serde_json::from_str(json).map_err(|e| HyprError::Protocol(format!("cursorpos: {e}")))?;
  let conv = |v: f64| {
    (v.is_finite() && v.abs() <= f64::from(i32::MAX))
      .then(|| v.round() as i32)
      .ok_or_else(|| HyprError::Protocol("cursorpos: coordinate out of range".into()))
  };
  Ok((conv(c.x)?, conv(c.y)?))
}

// Unknown fields (title, initialTitle, ...) are skipped by serde without
// being kept.
#[derive(Deserialize)]
struct WindowJson {
  #[serde(default)]
  address: Option<String>,
  #[serde(default)]
  class: Option<String>,
  #[serde(default, rename = "initialClass")]
  initial_class: Option<String>,
}

/// `j/activewindow` reply: `{}` (nothing focused) -> `Ok(None)`.
pub fn parse_active_window(json: &str) -> Result<Option<HyprWindow>, HyprError> {
  let w: WindowJson =
    serde_json::from_str(json).map_err(|e| HyprError::Protocol(format!("activewindow: {e}")))?;
  let Some(address) = w.address.as_deref().and_then(normalize_address) else {
    return Ok(None);
  };
  let class =
    w.class.as_deref().and_then(nonempty).or_else(|| w.initial_class.as_deref().and_then(nonempty));
  Ok(Some(HyprWindow { address, class }))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn addresses() {
    assert_eq!(normalize_address("55d5c0a3b2c0").as_deref(), Some("0x55d5c0a3b2c0"));
    assert_eq!(normalize_address("0x55D5C0A3B2C0").as_deref(), Some("0x55d5c0a3b2c0"));
    assert_eq!(normalize_address(""), None);
    assert_eq!(normalize_address("0x"), None);
    assert_eq!(normalize_address("zz"), None);
    assert_eq!(normalize_address("11112222333344445"), None);
  }

  #[test]
  fn events() {
    assert_eq!(
      parse_event("activewindow>>foot,~/src, with a comma"),
      Some(HyprEvent::ActiveWindow { class: Some("foot".into()) })
    );
    assert_eq!(parse_event("activewindow>>,"), Some(HyprEvent::ActiveWindow { class: None }));
    assert_eq!(
      parse_event("activewindowv2>>55d5c0a3b2c0"),
      Some(HyprEvent::ActiveWindowV2 { address: Some("0x55d5c0a3b2c0".into()) })
    );
    assert_eq!(parse_event("activewindowv2>>"), Some(HyprEvent::ActiveWindowV2 { address: None }));
    assert_eq!(parse_event("activewindowv2>>,"), Some(HyprEvent::ActiveWindowV2 { address: None }));
    assert_eq!(
      parse_event("closewindow>>55d5c0a3b2c0"),
      Some(HyprEvent::CloseWindow { address: Some("0x55d5c0a3b2c0".into()) })
    );
    assert_eq!(parse_event("workspace>>2"), None);
    assert_eq!(parse_event("garbage"), None);
  }

  #[test]
  fn cursorpos() {
    assert_eq!(parse_cursorpos(r#"{"x": 1234, "y": 567}"#).unwrap(), (1234, 567));
    assert_eq!(parse_cursorpos(r#"{"x": -10.4, "y": 3.6}"#).unwrap(), (-10, 4));
    assert!(parse_cursorpos(r#"{"x": 1e20, "y": 0}"#).is_err());
    assert!(parse_cursorpos("unknown request").is_err());
  }

  #[test]
  fn active_window_fixtures() {
    let w = parse_active_window(include_str!("../tests/fixtures/activewindow.json")).unwrap();
    assert_eq!(
      w,
      Some(HyprWindow { address: "0x5581f00ba2a0".into(), class: Some("foot".into()) })
    );
    let x = parse_active_window(include_str!("../tests/fixtures/activewindow-xwayland.json"))
      .unwrap()
      .unwrap();
    assert_eq!(x.class.as_deref(), Some("firefox"));
    assert_eq!(
      parse_active_window(include_str!("../tests/fixtures/activewindow-none.json")).unwrap(),
      None
    );
    assert_eq!(
      parse_cursorpos(include_str!("../tests/fixtures/cursorpos.json")).unwrap(),
      (1287, 642)
    );
    assert!(parse_active_window("ok").is_err());
  }

  #[test]
  fn event_fixture() {
    let evs: Vec<HyprEvent> =
      include_str!("../tests/fixtures/socket2.txt").lines().filter_map(parse_event).collect();
    assert_eq!(
      evs,
      vec![
        HyprEvent::ActiveWindow { class: Some("foot".into()) },
        HyprEvent::ActiveWindowV2 { address: Some("0x5581f00ba2a0".into()) },
        HyprEvent::ActiveWindow { class: Some("org.kde.kate".into()) },
        HyprEvent::ActiveWindowV2 { address: Some("0x5581f00c1e40".into()) },
        HyprEvent::CloseWindow { address: Some("0x5581f00c1e40".into()) },
        HyprEvent::ActiveWindow { class: None },
        HyprEvent::ActiveWindowV2 { address: None },
      ]
    );
  }
}
