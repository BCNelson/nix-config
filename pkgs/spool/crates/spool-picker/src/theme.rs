//! KDE colour scheme / font from `~/.config/kdeglobals`, with a fallback
//! palette when the file or a key is missing.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
  fn luminance(self) -> f32 {
    (0.2126 * self.0 as f32 + 0.7152 * self.1 as f32 + 0.0722 * self.2 as f32) / 255.0
  }

  /// Linear mix towards `other` by `t` (0..=1).
  pub fn mix(self, other: Rgb, t: f32) -> Rgb {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
    Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
  pub bg: Rgb,
  pub fg: Rgb,
  pub dim: Rgb,
  pub sel_bg: Rgb,
  pub sel_fg: Rgb,
  pub accent: Rgb,
  /// Error text (`[Colors:View] ForegroundNegative`).
  pub negative: Rgb,
  /// Font family from `[General] font=`; `None` = the embedded font.
  pub font_family: Option<String>,
  /// Font size in logical pixels.
  pub font_px: f32,
}

/// Family of the embedded UI font (a Noto Sans subset).
pub const EMBEDDED_FAMILY: &str = "Spool Sans";
const DEFAULT_FONT_PX: f32 = 10.0 * 96.0 / 72.0;

impl Theme {
  /// Breeze Dark-like palette.
  pub fn fallback_dark() -> Self {
    Self {
      bg: Rgb(0x23, 0x26, 0x29),
      fg: Rgb(0xfc, 0xfc, 0xfc),
      dim: Rgb(0xa1, 0xa9, 0xb1),
      sel_bg: Rgb(0x3d, 0xae, 0xe9),
      sel_fg: Rgb(0xfc, 0xfc, 0xfc),
      accent: Rgb(0x3d, 0xae, 0xe9),
      negative: Rgb(0xda, 0x44, 0x53),
      font_family: None,
      font_px: DEFAULT_FONT_PX,
    }
  }

  /// Breeze Light-like palette.
  pub fn fallback_light() -> Self {
    Self {
      bg: Rgb(0xff, 0xff, 0xff),
      fg: Rgb(0x23, 0x26, 0x29),
      dim: Rgb(0x70, 0x7d, 0x8a),
      sel_bg: Rgb(0x3d, 0xae, 0xe9),
      sel_fg: Rgb(0xff, 0xff, 0xff),
      accent: Rgb(0x3d, 0xae, 0xe9),
      negative: Rgb(0xda, 0x44, 0x53),
      font_family: None,
      font_px: DEFAULT_FONT_PX,
    }
  }

  #[cfg(test)]
  pub fn is_dark(&self) -> bool {
    self.bg.luminance() < 0.5
  }

  /// Load from `kdeglobals_path()`; fallback palette on any problem.
  pub fn load() -> Self {
    match kdeglobals_path().and_then(|p| std::fs::read_to_string(p).ok()) {
      Some(s) => parse_kdeglobals(&s),
      None => Self::fallback_dark(),
    }
  }
}

/// `$XDG_CONFIG_HOME/kdeglobals` or `$HOME/.config/kdeglobals`.
pub fn kdeglobals_path() -> Option<PathBuf> {
  config_dir().map(|d| d.join("kdeglobals"))
}

pub fn config_dir() -> Option<PathBuf> {
  if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
    let p = PathBuf::from(x);
    if p.is_absolute() {
      return Some(p);
    }
  }
  std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".config"))
}

fn parse_color(v: &str) -> Option<Rgb> {
  let v = v.trim();
  if let Some(hex) = v.strip_prefix('#') {
    if hex.len() == 6 {
      let n = u32::from_str_radix(hex, 16).ok()?;
      return Some(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    return None;
  }
  let parts: Vec<_> = v.split(',').map(str::trim).collect();
  if parts.len() != 3 && parts.len() != 4 {
    return None;
  }
  let c = |s: &str| s.parse::<u8>().ok();
  Some(Rgb(c(parts[0])?, c(parts[1])?, c(parts[2])?))
}

/// Qt font string: `family,pointSize,pixelSize,...`.
fn parse_font(v: &str) -> Option<(String, f32)> {
  let mut it = v.split(',');
  let family = it.next()?.trim();
  if family.is_empty() || family.chars().any(char::is_control) || family.len() > 128 {
    return None;
  }
  let pt: f32 = it.next()?.trim().parse().ok()?;
  let px_field: f32 = it.next().and_then(|s| s.trim().parse().ok()).unwrap_or(-1.0);
  let px = if pt > 0.0 { pt * 96.0 / 72.0 } else { px_field };
  if !(6.0..=48.0).contains(&px) {
    return None;
  }
  Some((family.to_string(), px))
}

/// Parse kdeglobals (INI). Unknown/missing keys fall back to the palette that
/// matches the scheme's background brightness.
pub fn parse_kdeglobals(s: &str) -> Theme {
  let mut section = String::new();
  let mut view_bg = None;
  let mut view_fg = None;
  let mut view_dim = None;
  let mut view_focus = None;
  let mut sel_bg = None;
  let mut sel_fg = None;
  let mut negative = None;
  let mut font = None;
  for line in s.lines() {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
      continue;
    }
    if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
      section = sec.to_string();
      continue;
    }
    let Some((k, v)) = line.split_once('=') else { continue };
    // KConfig allows `key[$e]=` style flags; strip them.
    let k = k.split('[').next().unwrap_or(k).trim();
    match (section.as_str(), k) {
      ("Colors:View", "BackgroundNormal") => view_bg = parse_color(v),
      ("Colors:View", "ForegroundNormal") => view_fg = parse_color(v),
      ("Colors:View", "ForegroundInactive") => view_dim = parse_color(v),
      ("Colors:View", "DecorationFocus") => view_focus = parse_color(v),
      ("Colors:View", "ForegroundNegative") => negative = parse_color(v),
      ("Colors:Selection", "BackgroundNormal") => sel_bg = parse_color(v),
      ("Colors:Selection", "ForegroundNormal") => sel_fg = parse_color(v),
      ("General", "font") => font = parse_font(v),
      _ => {}
    }
  }
  let mut t = match view_bg {
    Some(bg) if bg.luminance() >= 0.5 => Theme::fallback_light(),
    _ => Theme::fallback_dark(),
  };
  if let Some(c) = view_bg {
    t.bg = c;
  }
  if let Some(c) = view_fg {
    t.fg = c;
  }
  t.dim = match (view_dim, view_bg.or(view_fg)) {
    (Some(c), _) => c,
    // Custom bg/fg without an explicit inactive colour: derive one.
    (None, Some(_)) => t.fg.mix(t.bg, 0.4),
    (None, None) => t.dim,
  };
  if let Some(c) = sel_bg {
    t.sel_bg = c;
  }
  if let Some(c) = sel_fg {
    t.sel_fg = c;
  }
  t.accent = view_focus.or(sel_bg).unwrap_or(t.accent);
  if let Some(c) = negative {
    t.negative = c;
  }
  if let Some((family, px)) = font {
    t.font_px = px;
    // Noto Sans is KDE's default and what the embedded font is cut from;
    // keep the embedded copy so no fontconfig lookup is needed.
    if family != "Noto Sans" {
      t.font_family = Some(family);
    }
  }
  t
}

#[cfg(test)]
mod tests {
  use super::*;

  const BREEZE_DARK: &str = "\
[ColorEffects:Disabled]
Color=56,56,56

[Colors:Selection]
BackgroundAlternate=30,87,116
BackgroundNormal=61,174,233
ForegroundNormal=252,252,252

[Colors:View]
BackgroundAlternate=35,38,41
BackgroundNormal=27,30,32
DecorationFocus=61,174,233
ForegroundInactive=161,169,177
ForegroundNegative=218,68,83
ForegroundNormal=252,252,252

[General]
ColorScheme=BreezeDark
font=Noto Sans,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1
";

  #[test]
  fn parses_breeze_dark() {
    let t = parse_kdeglobals(BREEZE_DARK);
    assert_eq!(t.bg, Rgb(27, 30, 32));
    assert_eq!(t.fg, Rgb(252, 252, 252));
    assert_eq!(t.dim, Rgb(161, 169, 177));
    assert_eq!(t.sel_bg, Rgb(61, 174, 233));
    assert_eq!(t.sel_fg, Rgb(252, 252, 252));
    assert_eq!(t.accent, Rgb(61, 174, 233));
    assert_eq!(t.negative, Rgb(218, 68, 83));
    assert!(t.is_dark());
    assert_eq!(t.font_family, None, "Noto Sans maps to the embedded font");
    assert!((t.font_px - 13.333).abs() < 0.01);
  }

  #[test]
  fn light_scheme_and_custom_font() {
    let t = parse_kdeglobals(
      "[Colors:View]\nBackgroundNormal=#fafafa\nForegroundNormal=20,20,20,255\n\
       [General]\nfont=Inter,12,-1,5,50,0,0,0,0,0\n",
    );
    assert_eq!(t.bg, Rgb(0xfa, 0xfa, 0xfa));
    assert_eq!(t.fg, Rgb(20, 20, 20));
    assert!(!t.is_dark());
    // Selection falls back to the light palette.
    assert_eq!(t.sel_bg, Theme::fallback_light().sel_bg);
    assert_eq!(t.font_family.as_deref(), Some("Inter"));
    assert!((t.font_px - 16.0).abs() < 0.01);
  }

  #[test]
  fn garbage_falls_back() {
    assert_eq!(parse_kdeglobals(""), Theme::fallback_dark());
    let t = parse_kdeglobals(
      "[Colors:View]\nBackgroundNormal=300,1,2\nForegroundNormal=a,b,c\n[General]\nfont=,99999\n\
       [Colors:Selection\nBackgroundNormal=1,2,3\nnot a line\n",
    );
    assert_eq!(t.bg, Theme::fallback_dark().bg);
    assert_eq!(t.fg, Theme::fallback_dark().fg);
    assert_eq!(t.font_px, Theme::fallback_dark().font_px);
    // Pixel-size fonts (point size -1) use the pixel field.
    assert_eq!(parse_font("X,-1,15,5"), Some(("X".into(), 15.0)));
    assert_eq!(parse_font("X,200,-1"), None);
  }

  #[test]
  fn kconfig_flags_and_sections() {
    let t = parse_kdeglobals(
      "[Colors:Window]\nBackgroundNormal=1,1,1\n[Colors:View]\nBackgroundNormal[$e]=9,9,9\n",
    );
    assert_eq!(t.bg, Rgb(9, 9, 9));
  }
}
