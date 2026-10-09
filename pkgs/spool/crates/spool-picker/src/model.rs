//! Pure helpers that turn `ItemPreview`s into display strings.

use spool_proto::{ItemPreview, PreviewKind};

use crate::sanitize::{PREVIEW_CHARS, sanitize_line};

/// Compact age: `now`, `42s`, `5m`, `3h`, `2d`, `6w`, `1y`.
pub fn relative_time(now_ms: u64, then_ms: u64) -> String {
  let s = now_ms.saturating_sub(then_ms) / 1000;
  match s {
    0..=4 => "now".into(),
    5..=59 => format!("{s}s"),
    60..=3599 => format!("{}m", s / 60),
    3600..=86_399 => format!("{}h", s / 3600),
    86_400..=1_209_599 => format!("{}d", s / 86_400),
    1_209_600..=31_535_999 => format!("{}w", s / 604_800),
    _ => format!("{}y", s / 31_536_000),
  }
}

/// Short type label for the badge.
pub fn badge(p: &ItemPreview) -> String {
  match p.kind {
    PreviewKind::Text => "TEXT".into(),
    PreviewKind::Html => "HTML".into(),
    PreviewKind::Files => "FILES".into(),
    PreviewKind::Image => match image_mime(p) {
      Some("image/png") => "PNG".into(),
      Some("image/jpeg") => "JPEG".into(),
      Some("image/webp") => "WEBP".into(),
      _ => "IMAGE".into(),
    },
    PreviewKind::Other => "DATA".into(),
  }
}

/// The image representation to request a thumbnail for.
pub fn image_mime(p: &ItemPreview) -> Option<&'static str> {
  const ORDER: [&str; 3] = ["image/png", "image/webp", "image/jpeg"];
  ORDER.into_iter().find(|m| p.mimes.iter().any(|x| x == m))
}

/// Sanitized one-line preview text (falls back to a size label for
/// previewless items).
pub fn preview_text(p: &ItemPreview) -> String {
  let t = sanitize_line(&p.preview, PREVIEW_CHARS);
  if !t.trim().is_empty() {
    return t;
  }
  match p.kind {
    PreviewKind::Image => format!("Image · {}", human_size(p.total_size)),
    _ => format!("({})", human_size(p.total_size)),
  }
}

pub fn human_size(n: u64) -> String {
  match n {
    0..=1023 => format!("{n} B"),
    1024..=1_048_575 => format!("{:.1} KiB", n as f64 / 1024.0),
    _ => format!("{:.1} MiB", n as f64 / 1_048_576.0),
  }
}

pub fn now_ms() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn p(kind: PreviewKind, mimes: &[&str], preview: &str) -> ItemPreview {
    ItemPreview {
      id: 1,
      kind,
      preview: preview.into(),
      mimes: mimes.iter().map(|s| s.to_string()).collect(),
      source_app: None,
      created_unix_ms: 0,
      last_used_unix_ms: 0,
      pinned: false,
      tags: vec![],
      total_size: 2048,
    }
  }

  #[test]
  fn ages() {
    let now = 100_000_000_000;
    assert_eq!(relative_time(now, now), "now");
    assert_eq!(relative_time(now, now + 5000), "now", "clock skew");
    assert_eq!(relative_time(now, now - 42_000), "42s");
    assert_eq!(relative_time(now, now - 5 * 60_000), "5m");
    assert_eq!(relative_time(now, now - 3 * 3_600_000), "3h");
    assert_eq!(relative_time(now, now - 2 * 86_400_000), "2d");
    assert_eq!(relative_time(now, now - 21 * 86_400_000), "3w");
    assert_eq!(relative_time(now, now - 400 * 86_400_000), "1y");
  }

  #[test]
  fn badges_and_mimes() {
    assert_eq!(badge(&p(PreviewKind::Text, &["text/plain"], "")), "TEXT");
    assert_eq!(badge(&p(PreviewKind::Image, &["image/jpeg", "image/png"], "")), "PNG");
    assert_eq!(image_mime(&p(PreviewKind::Image, &["image/jpeg"], "")), Some("image/jpeg"));
    assert_eq!(image_mime(&p(PreviewKind::Image, &["image/gif"], "")), None);
    assert_eq!(badge(&p(PreviewKind::Image, &["image/gif"], "")), "IMAGE");
  }

  #[test]
  fn previews_are_sanitized() {
    assert_eq!(preview_text(&p(PreviewKind::Text, &[], "a\u{202E}b\n")), "a⟪RLO⟫b␊");
    assert_eq!(preview_text(&p(PreviewKind::Image, &[], "")), "Image · 2.0 KiB");
    assert_eq!(preview_text(&p(PreviewKind::Other, &[], "  ")), "(2.0 KiB)");
  }
}
