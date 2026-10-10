//! Pure helpers for editing sessions: which mime goes into the temp file,
//! its extension, image sniffing and the plain text derived from edited
//! HTML. No I/O; never logs content.

use spool_core::item::TEXT_MIMES;

/// `source_app` of items saved from an external editor.
pub const EDITOR_SOURCE_APP: &str = "spool.editor";

/// The text mime edited files are written and stored as (text is always
/// edited as UTF-8).
pub const UTF8_TEXT: &str = "text/plain;charset=utf-8";

/// How a saved file becomes representations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
  /// Plain text (any of `TEXT_MIMES`, or another `text/*` but HTML):
  /// must be UTF-8; stored as that one text rep (+ aliases for plain text).
  Text,
  /// `text/html`: stored as HTML plus plain text derived from it.
  Html,
  /// `image/*`: the bytes are sniffed; stored as that image rep only.
  Image,
  /// Anything else: stored as is.
  Other,
}

fn essence(mime: &str) -> String {
  mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// The mime an item's representation `mime` is edited as: the text
/// variants (`UTF8_STRING`, `text/plain`, ...) all become [`UTF8_TEXT`];
/// anything else keeps its type.
pub fn edit_mime(mime: &str) -> String {
  let m = mime.trim();
  if TEXT_MIMES.iter().any(|t| t.eq_ignore_ascii_case(m)) || essence(m) == "text/plain" {
    UTF8_TEXT.to_owned()
  } else {
    m.to_owned()
  }
}

pub fn kind_of(edit_mime: &str) -> EditKind {
  let e = essence(edit_mime);
  if e == "text/html" || e == "application/xhtml+xml" {
    EditKind::Html
  } else if e.starts_with("text/") || TEXT_MIMES.iter().any(|t| t.eq_ignore_ascii_case(edit_mime)) {
    EditKind::Text
  } else if e.starts_with("image/") {
    EditKind::Image
  } else {
    EditKind::Other
  }
}

/// File extension for the temp file (editors pick their mode from it).
pub fn file_ext(edit_mime: &str) -> &'static str {
  match essence(edit_mime).as_str() {
    "text/plain" | "text/uri-list" => "txt",
    "text/html" | "application/xhtml+xml" => "html",
    "text/markdown" | "text/x-markdown" => "md",
    "text/csv" => "csv",
    "text/css" => "css",
    "text/xml" | "application/xml" => "xml",
    "application/json" => "json",
    "image/png" => "png",
    "image/jpeg" => "jpg",
    "image/webp" => "webp",
    "image/gif" => "gif",
    "image/bmp" => "bmp",
    "image/svg+xml" => "svg",
    e if e.starts_with("text/") => "txt",
    _ => "bin",
  }
}

/// The image type of `data` from its magic bytes, if it is one we know.
pub fn sniff_image(data: &[u8]) -> Option<&'static str> {
  if data.starts_with(b"\x89PNG\r\n\x1a\n") {
    Some("image/png")
  } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
    Some("image/jpeg")
  } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
    Some("image/webp")
  } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
    Some("image/gif")
  } else if data.starts_with(b"BM") && data.len() > 14 {
    Some("image/bmp")
  } else {
    None
  }
}

/// Image types a saved image may become (what the picker can thumbnail),
/// besides the type that was being edited.
pub const SAVE_IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

/// The mime a saved image is stored as: the sniffed type, if it is the
/// edited type or one of [`SAVE_IMAGE_MIMES`]; `None` = not acceptable.
pub fn saved_image_mime(edit_mime: &str, data: &[u8]) -> Option<&'static str> {
  let sniffed = sniff_image(data)?;
  (sniffed == essence(edit_mime) || SAVE_IMAGE_MIMES.contains(&sniffed)).then_some(sniffed)
}

const BLOCK_TAGS: &[&str] = &[
  "address",
  "article",
  "aside",
  "blockquote",
  "dd",
  "div",
  "dl",
  "dt",
  "figcaption",
  "figure",
  "footer",
  "form",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "header",
  "hr",
  "main",
  "nav",
  "ol",
  "p",
  "pre",
  "section",
  "table",
  "tr",
  "ul",
];

/// Elements whose content is not text.
const SKIP_TAGS: &[&str] = &["script", "style", "head", "template", "noscript", "title"];

fn entity(name: &str) -> Option<char> {
  if let Some(num) = name.strip_prefix('#') {
    let n = match num.strip_prefix(['x', 'X']) {
      Some(hex) => u32::from_str_radix(hex, 16).ok()?,
      None => num.parse().ok()?,
    };
    return char::from_u32(n).filter(|c| *c != '\0');
  }
  Some(match name {
    "amp" => '&',
    "lt" => '<',
    "gt" => '>',
    "quot" => '"',
    "apos" => '\'',
    "nbsp" => ' ',
    "ndash" => '\u{2013}',
    "mdash" => '\u{2014}',
    "hellip" => '\u{2026}',
    "lsquo" => '\u{2018}',
    "rsquo" => '\u{2019}',
    "ldquo" => '\u{201c}',
    "rdquo" => '\u{201d}',
    "bull" => '\u{2022}',
    "middot" => '\u{b7}',
    "copy" => '\u{a9}',
    "reg" => '\u{ae}',
    "trade" => '\u{2122}',
    "euro" => '\u{20ac}',
    "times" => '\u{d7}',
    _ => return None,
  })
}

/// Plain text for an edited HTML item's text representation, so pasting
/// into plain-text targets works: tags dropped (script/style/head content
/// too), block elements and `<br>` become line breaks, list items get a
/// `- ` marker, whitespace collapsed outside `<pre>`, common entities
/// decoded. Not a renderer; good enough for clipboard snippets.
pub fn html_to_text(html: &str) -> String {
  let mut out = String::with_capacity(html.len() / 2);
  let mut skip: Option<String> = None;
  let mut pre = 0usize;
  let mut rest = html;
  let mut pending_space = false;

  fn newline(out: &mut String, pending_space: &mut bool) {
    *pending_space = false;
    while out.ends_with(' ') {
      out.pop();
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
      out.push('\n');
    }
  }

  while !rest.is_empty() {
    if let Some(after) = rest.strip_prefix("<!--") {
      rest = after.find("-->").map_or("", |i| &after[i + 3..]);
      continue;
    }
    if rest.starts_with('<') {
      let (tag, after) = match rest.find('>') {
        Some(end) => (&rest[1..end], &rest[end + 1..]),
        None => (&rest[1..], ""),
      };
      rest = after;
      let closing = tag.starts_with('/');
      let name: String = tag
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
      if let Some(s) = &skip {
        if closing && *s == name {
          skip = None;
        }
        continue;
      }
      if name.is_empty() {
        continue; // `<!DOCTYPE`, `<?xml`, stray `<`
      }
      if SKIP_TAGS.contains(&name.as_str()) && !closing && !tag.ends_with('/') {
        skip = Some(name);
        continue;
      }
      match name.as_str() {
        "br" => {
          while out.ends_with(' ') {
            out.pop();
          }
          out.push('\n');
          pending_space = false;
        }
        "li" if !closing => {
          newline(&mut out, &mut pending_space);
          out.push_str("- ");
        }
        "td" | "th" if !closing => pending_space = true,
        "pre" => {
          newline(&mut out, &mut pending_space);
          pre = if closing { pre.saturating_sub(1) } else { pre + 1 };
        }
        n if BLOCK_TAGS.contains(&n) => newline(&mut out, &mut pending_space),
        _ => {}
      }
      continue;
    }
    // Text up to the next tag.
    let end = rest.find('<').unwrap_or(rest.len());
    let text = &rest[..end];
    rest = &rest[end..];
    if skip.is_some() {
      continue;
    }
    let mut t = text;
    while !t.is_empty() {
      let (c, len) = if let Some(after) = t.strip_prefix('&') {
        match after.find(';').filter(|&i| i <= 10).and_then(|i| entity(&after[..i]).map(|c| (c, i)))
        {
          Some((c, i)) => (c, i + 2),
          None => ('&', 1),
        }
      } else {
        let c = t.chars().next().expect("non-empty");
        (c, c.len_utf8())
      };
      t = &t[len..];
      if pre > 0 {
        out.push(c);
        continue;
      }
      if c.is_whitespace() && c != '\u{a0}' {
        pending_space = true;
        continue;
      }
      if pending_space && !out.is_empty() && !out.ends_with('\n') && !out.ends_with(' ') {
        out.push(' ');
      }
      pending_space = false;
      out.push(if c == '\u{a0}' { ' ' } else { c });
    }
  }
  let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
  let mut text = String::with_capacity(out.len());
  let mut blank = 0;
  for l in lines {
    if l.is_empty() {
      blank += 1;
      if blank > 1 {
        continue;
      }
    } else {
      blank = 0;
    }
    text.push_str(l);
    text.push('\n');
  }
  text.trim().to_owned()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn edit_mimes_and_kinds() {
    for m in ["UTF8_STRING", "TEXT", "STRING", "text/plain", "text/plain;charset=utf-8"] {
      assert_eq!(edit_mime(m), UTF8_TEXT, "{m}");
    }
    assert_eq!(edit_mime("text/html"), "text/html");
    assert_eq!(edit_mime("image/png"), "image/png");
    assert_eq!(kind_of(UTF8_TEXT), EditKind::Text);
    assert_eq!(kind_of("text/uri-list"), EditKind::Text);
    assert_eq!(kind_of("text/html;charset=utf-8"), EditKind::Html);
    assert_eq!(kind_of("image/webp"), EditKind::Image);
    assert_eq!(kind_of("application/json"), EditKind::Other);
  }

  #[test]
  fn extensions() {
    assert_eq!(file_ext(UTF8_TEXT), "txt");
    assert_eq!(file_ext("text/html"), "html");
    assert_eq!(file_ext("TEXT/HTML; charset=utf-8"), "html");
    assert_eq!(file_ext("image/png"), "png");
    assert_eq!(file_ext("image/jpeg"), "jpg");
    assert_eq!(file_ext("image/webp"), "webp");
    assert_eq!(file_ext("text/x-rust"), "txt");
    assert_eq!(file_ext("application/x-thing"), "bin");
  }

  #[test]
  fn image_sniffing() {
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    let jpg = [0xff, 0xd8, 0xff, 0xe0, 0, 0x10];
    let webp = b"RIFF\x24\0\0\0WEBPVP8 ";
    assert_eq!(sniff_image(png), Some("image/png"));
    assert_eq!(sniff_image(&jpg), Some("image/jpeg"));
    assert_eq!(sniff_image(webp), Some("image/webp"));
    assert_eq!(sniff_image(b"GIF89a....."), Some("image/gif"));
    assert_eq!(sniff_image(b"hello"), None);
    assert_eq!(sniff_image(b"RIFF\0\0\0\0WAVE"), None);
    // The extension is not trusted: a PNG saved over a .jpg is a PNG.
    assert_eq!(saved_image_mime("image/jpeg", png), Some("image/png"));
    assert_eq!(saved_image_mime("image/png", b"not an image"), None);
    // GIF only if GIF was being edited.
    assert_eq!(saved_image_mime("image/png", b"GIF89a....."), None);
    assert_eq!(saved_image_mime("image/gif", b"GIF89a....."), Some("image/gif"));
  }

  #[test]
  fn html_to_text_basics() {
    assert_eq!(html_to_text("<b>Hello</b>,   <i>world</i>!"), "Hello, world!");
    assert_eq!(
      html_to_text(
        "<html><head><title>T</title><style>p{}</style></head><body><p>One</p><p>Two<br>Three</p></body></html>"
      ),
      "One\n\nTwo\nThree"
    );
    assert_eq!(html_to_text("<ul><li>a</li><li>b &amp; c</li></ul>"), "- a\n- b & c");
    assert_eq!(html_to_text("x &lt;y&gt; &#65;&#x42; &nbsp;z &bogus; &"), "x <y> AB z &bogus; &");
    assert_eq!(html_to_text("<pre>  keep\n    this</pre>after"), "keep\n    this\nafter");
    assert_eq!(html_to_text("a<!-- secret --><script>var x = '<b>'</script>b"), "ab");
    assert_eq!(html_to_text("<table><tr><td>1</td><td>2</td></tr></table>"), "1 2");
    assert_eq!(html_to_text(""), "");
    assert_eq!(html_to_text("<div><br/></div>"), "");
    // Unterminated tags / comments do not panic.
    assert_eq!(html_to_text("text <b"), "text");
    assert_eq!(html_to_text("<"), "");
    assert_eq!(html_to_text("a <!-- open"), "a");
    assert_eq!(html_to_text("é<b>ü</b>"), "éü");
  }
}
