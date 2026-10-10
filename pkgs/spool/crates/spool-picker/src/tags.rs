//! The inline tag editor (Ctrl+T): key handling and the optimistic tag
//! edits, kept free of Wayland/Slint so they can be unit-tested.
//!
//! Keys while the editor is open (`App` routes them here):
//! * Enter adds the typed tag, normalized (trimmed) and checked by
//!   [`spool_proto::check_tag`], the daemon's own rule. Enter never selects or
//!   pastes while the editor is open.
//! * Backspace on an empty field selects the last chip; Backspace or Del on
//!   a selected chip removes it (two steps, so a held Backspace can't wipe
//!   every tag). Left/Right on an empty field move the chip selection.
//! * Anything else goes to the text field and clears the chip selection.

use spool_proto::{ItemPreview, WireItemId, check_tag};

/// Keys the editor interprets itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKey {
  Enter,
  Backspace,
  Delete,
  Left,
  Right,
}

/// What `App` should do with a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
  /// Not ours: the text field gets the key.
  Field,
  /// Handled here (chip selection changed, or nothing to do).
  Handled,
  /// Add this tag (already trimmed and valid) and clear the field.
  Add(String),
  /// Remove this tag.
  Remove(String),
  /// Rejected input: show this message (never contains the tag).
  Invalid(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEditor {
  /// The item being edited (stays the same while the editor is open).
  pub id: WireItemId,
  /// Selected chip (index into the item's tags), for keyboard removal.
  pub chip: Option<usize>,
}

impl TagEditor {
  pub fn new(id: WireItemId) -> Self {
    Self { id, chip: None }
  }

  /// Handle `key`. `field` is the text typed so far, `tags` the item's
  /// current tags (in display order).
  pub fn key(&mut self, key: TagKey, repeat: bool, field: &str, tags: &[String]) -> Step {
    if self.chip.is_some_and(|c| c >= tags.len()) {
      self.chip = None;
    }
    let empty = field.is_empty();
    match key {
      TagKey::Enter => {
        if repeat || empty {
          return Step::Handled;
        }
        match check_tag(field) {
          Ok(t) => {
            self.chip = None;
            Step::Add(t.to_string())
          }
          Err(e) => Step::Invalid(e.message()),
        }
      }
      TagKey::Backspace if !empty => {
        self.chip = None;
        Step::Field
      }
      TagKey::Backspace | TagKey::Delete => match self.chip {
        // A held key removes at most the one chip it was pressed for.
        Some(_) if repeat => Step::Handled,
        Some(c) => {
          self.chip = None;
          Step::Remove(tags[c].clone())
        }
        None if key == TagKey::Delete => Step::Field,
        None if repeat => Step::Handled,
        None => {
          self.chip = tags.len().checked_sub(1);
          Step::Handled
        }
      },
      TagKey::Left if empty && !tags.is_empty() => {
        self.chip = Some(self.chip.map_or(tags.len() - 1, |c| c.saturating_sub(1)));
        Step::Handled
      }
      TagKey::Right if self.chip.is_some() => {
        // Past the last chip: back to the field.
        self.chip = self.chip.map(|c| c + 1).filter(|&c| c < tags.len());
        Step::Handled
      }
      TagKey::Left | TagKey::Right => Step::Field,
    }
  }

  /// A key went to the text field: drop the chip selection.
  pub fn typed(&mut self) {
    self.chip = None;
  }
}

/// Optimistically apply a tag edit to a list entry. `false`: nothing changes
/// (already tagged / not tagged), so there is nothing to send.
pub fn apply(p: &mut ItemPreview, tag: &str, on: bool) -> bool {
  let pos = p.tags.iter().position(|t| t == tag);
  match (on, pos) {
    (true, None) => {
      p.tags.push(tag.to_string());
      true
    }
    (false, Some(i)) => {
      p.tags.remove(i);
      true
    }
    _ => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use spool_proto::{MAX_TAG_CHARS, PreviewKind, TagError};

  fn tags(t: &[&str]) -> Vec<String> {
    t.iter().map(|s| s.to_string()).collect()
  }

  fn item(t: &[&str]) -> ItemPreview {
    ItemPreview {
      id: 1,
      kind: PreviewKind::Text,
      preview: "x".into(),
      mimes: vec![],
      source_app: None,
      created_unix_ms: 0,
      last_used_unix_ms: 0,
      pinned: false,
      tags: tags(t),
      total_size: 1,
    }
  }

  #[test]
  fn enter_adds_trimmed_valid_tags_only() {
    let mut e = TagEditor::new(1);
    let t = tags(&[]);
    assert_eq!(e.key(TagKey::Enter, false, "  work ", &t), Step::Add("work".into()));
    assert_eq!(e.key(TagKey::Enter, false, "", &t), Step::Handled, "empty field: no-op");
    assert_eq!(e.key(TagKey::Enter, true, "work", &t), Step::Handled, "repeat: no-op");
    assert_eq!(e.key(TagKey::Enter, false, "   ", &t), Step::Invalid(TagError::Empty.message()));
    assert_eq!(e.key(TagKey::Enter, false, "a/b", &t), Step::Invalid(TagError::BadChar.message()));
    assert_eq!(
      e.key(TagKey::Enter, false, &"x".repeat(MAX_TAG_CHARS + 1), &t),
      Step::Invalid(TagError::TooLong.message())
    );
    // Surrounding spaces don't count toward the limit.
    let max = format!(" {} ", "x".repeat(MAX_TAG_CHARS));
    assert_eq!(e.key(TagKey::Enter, false, &max, &t), Step::Add("x".repeat(MAX_TAG_CHARS)));
  }

  #[test]
  fn invalid_messages_never_echo_the_tag() {
    let mut e = TagEditor::new(1);
    for bad in ["secret/word", "secret\u{7}word", &"secretword".repeat(7)] {
      match e.key(TagKey::Enter, false, bad, &[]) {
        Step::Invalid(m) => assert!(!m.contains("secret"), "{m}"),
        other => panic!("{other:?}"),
      }
    }
  }

  #[test]
  fn backspace_selects_then_removes_the_last_chip() {
    let mut e = TagEditor::new(1);
    let t = tags(&["a", "b", "c"]);
    // Text in the field: plain backspace.
    assert_eq!(e.key(TagKey::Backspace, false, "x", &t), Step::Field);
    assert_eq!(e.key(TagKey::Backspace, false, "", &t), Step::Handled);
    assert_eq!(e.chip, Some(2));
    // A held key never removes it.
    assert_eq!(e.key(TagKey::Backspace, true, "", &t), Step::Handled);
    assert_eq!(e.key(TagKey::Backspace, false, "", &t), Step::Remove("c".into()));
    assert_eq!(e.chip, None);
    // Holding Backspace on an empty field doesn't select either.
    assert_eq!(e.key(TagKey::Backspace, true, "", &tags(&["a", "b"])), Step::Handled);
    assert_eq!(e.chip, None);
    // No tags: nothing to select.
    assert_eq!(e.key(TagKey::Backspace, false, "", &[]), Step::Handled);
    assert_eq!(e.chip, None);
  }

  #[test]
  fn arrows_choose_a_chip_and_del_removes_it() {
    let mut e = TagEditor::new(1);
    let t = tags(&["a", "b", "c"]);
    assert_eq!(e.key(TagKey::Left, false, "typed", &t), Step::Field, "cursor movement");
    assert_eq!(e.key(TagKey::Right, false, "", &t), Step::Field);
    assert_eq!(e.key(TagKey::Left, false, "", &t), Step::Handled);
    assert_eq!(e.chip, Some(2));
    e.key(TagKey::Left, false, "", &t);
    e.key(TagKey::Left, false, "", &t);
    e.key(TagKey::Left, false, "", &t);
    assert_eq!(e.chip, Some(0), "stops at the first chip");
    e.key(TagKey::Right, false, "", &t);
    assert_eq!(e.chip, Some(1));
    assert_eq!(e.key(TagKey::Delete, true, "", &t), Step::Handled, "held Del: no-op");
    assert_eq!(e.key(TagKey::Delete, false, "", &t), Step::Remove("b".into()));
    assert_eq!(e.chip, None);
    // Del without a selected chip edits the field (never deletes the item).
    assert_eq!(e.key(TagKey::Delete, false, "", &t), Step::Field);
    // Right past the last chip returns to the field.
    e.key(TagKey::Left, false, "", &t);
    assert_eq!(e.key(TagKey::Right, false, "", &t), Step::Handled);
    assert_eq!(e.chip, None);
    // Typing clears the selection; a stale index is dropped.
    e.key(TagKey::Left, false, "", &t);
    e.typed();
    assert_eq!(e.chip, None);
    e.chip = Some(5);
    assert_eq!(e.key(TagKey::Delete, false, "", &t), Step::Field);
  }

  #[test]
  fn optimistic_apply() {
    let mut p = item(&["a"]);
    assert!(apply(&mut p, "b", true));
    assert_eq!(p.tags, ["a", "b"]);
    assert!(!apply(&mut p, "b", true), "already tagged: nothing to send");
    assert!(apply(&mut p, "a", false));
    assert_eq!(p.tags, ["b"]);
    assert!(!apply(&mut p, "a", false));
    assert!(!apply(&mut p, "B", false), "tags are case-sensitive");
  }
}
