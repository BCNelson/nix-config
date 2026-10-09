//! Input validation for values arriving from the KWin script over D-Bus.
//!
//! These checks keep garbage out of logs, the store and the picker; they are
//! not a security boundary (see the crate docs).

use crate::{MAX_COORD, MAX_ID_LEN};

/// Why an argument was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
  #[error("app id too long")]
  AppIdTooLong,
  #[error("app id contains control characters")]
  AppIdControl,
  #[error("window id too long")]
  WindowIdTooLong,
  #[error("window id is not a UUID")]
  WindowIdFormat,
}

/// Validates an app id (`desktopFileName` or `resourceClass`).
/// Empty means "unknown" and maps to `None`.
pub fn app_id(raw: &str) -> Result<Option<String>, Invalid> {
  if raw.len() > MAX_ID_LEN {
    return Err(Invalid::AppIdTooLong);
  }
  if raw.chars().any(char::is_control) {
    return Err(Invalid::AppIdControl);
  }
  let trimmed = raw.trim();
  Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

/// Validates a window id (`Window.internalId.toString()`, a `QUuid` such as
/// `{0c8f9a0e-3b9c-4d4c-8a8e-2f1d7a3b5c6d}`). Empty maps to `None`.
///
/// The id is normalised to lowercase without braces so that ids from
/// `ActiveWindow` and `Show` compare equal regardless of formatting.
pub fn window_id(raw: &str) -> Result<Option<String>, Invalid> {
  if raw.len() > MAX_ID_LEN {
    return Err(Invalid::WindowIdTooLong);
  }
  if raw.is_empty() {
    return Ok(None);
  }
  let inner = raw.strip_prefix('{').and_then(|s| s.strip_suffix('}')).unwrap_or(raw);
  let groups: Vec<&str> = inner.split('-').collect();
  let lens = [8, 4, 4, 4, 12];
  let ok = groups.len() == lens.len()
    && groups
      .iter()
      .zip(lens)
      .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()));
  if !ok {
    return Err(Invalid::WindowIdFormat);
  }
  Ok(Some(inner.to_ascii_lowercase()))
}

/// Cursor position: `None` if either coordinate is outside
/// `-MAX_COORD..=MAX_COORD` (the picker then centres itself).
pub fn cursor(x: i32, y: i32) -> Option<(i32, i32)> {
  let ok = |v: i32| (-MAX_COORD..=MAX_COORD).contains(&v);
  (ok(x) && ok(y)).then_some((x, y))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn app_ids() {
    assert_eq!(app_id(""), Ok(None));
    assert_eq!(app_id("   "), Ok(None));
    assert_eq!(app_id("org.kde.konsole"), Ok(Some("org.kde.konsole".into())));
    assert_eq!(app_id("Alacritty"), Ok(Some("Alacritty".into())));
    assert_eq!(app_id(&"a".repeat(MAX_ID_LEN)).map(|s| s.map(|s| s.len())), Ok(Some(MAX_ID_LEN)));
    assert_eq!(app_id(&"a".repeat(MAX_ID_LEN + 1)), Err(Invalid::AppIdTooLong));
    assert_eq!(app_id("evil\napp"), Err(Invalid::AppIdControl));
    assert_eq!(app_id("evil\u{1b}[31m"), Err(Invalid::AppIdControl));
    assert_eq!(app_id("a\u{0}b"), Err(Invalid::AppIdControl));
  }

  #[test]
  fn window_ids() {
    let q = "{0C8F9A0E-3B9C-4D4C-8A8E-2F1D7A3B5C6D}";
    let want = Some("0c8f9a0e-3b9c-4d4c-8a8e-2f1d7a3b5c6d".to_string());
    assert_eq!(window_id(q), Ok(want.clone()));
    assert_eq!(window_id("0c8f9a0e-3b9c-4d4c-8a8e-2f1d7a3b5c6d"), Ok(want));
    assert_eq!(window_id(""), Ok(None));
    assert_eq!(window_id("{}"), Err(Invalid::WindowIdFormat));
    assert_eq!(window_id("12345"), Err(Invalid::WindowIdFormat));
    assert_eq!(window_id("{0c8f9a0e-3b9c-4d4c-8a8e-2f1d7a3b5c6z}"), Err(Invalid::WindowIdFormat));
    assert_eq!(window_id("{0c8f9a0e-3b9c-4d4c-8a8e2f1d7a3b5c6d0}"), Err(Invalid::WindowIdFormat));
    assert_eq!(window_id(&"a".repeat(MAX_ID_LEN + 1)), Err(Invalid::WindowIdTooLong));
  }

  #[test]
  fn cursors() {
    assert_eq!(cursor(0, 0), Some((0, 0)));
    assert_eq!(cursor(-1920, 1080), Some((-1920, 1080)));
    assert_eq!(cursor(MAX_COORD, -MAX_COORD), Some((MAX_COORD, -MAX_COORD)));
    assert_eq!(cursor(MAX_COORD + 1, 0), None);
    assert_eq!(cursor(0, i32::MIN), None);
  }
}
