//! The picker side of the daemon <-> picker socketpair.
//!
//! * `Channel::connect` does the `Hello` handshake (the picker speaks first),
//!   then starts a reader thread (frames -> calloop channel) and a writer
//!   thread (so a slow daemon never blocks the UI thread).
//! * `Correlator` numbers `Query`/`Thumb`/edit requests (`seq`, picker
//!   protocol v3) and matches the daemon's answers by `seq`, in any order: a
//!   page whose `seq` is not the latest query is stale and dropped; an error
//!   for an edit (`Pin`/`Delete`/`Tag`, answered only on failure) names the
//!   item it was for.

use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, BufWriter, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;

use anyhow::{Context, bail};
use smithay_client_toolkit::reexports::calloop::channel as cchan;
use spool_proto::{
  CursorPos, FrameError, Hello, ItemPreview, PickerErrorCode, PickerEvt, PickerReq, QueryFilters,
  UnlockFailReason, UnlockPrompt, UnlockProvider, WireItemId, read_frame, write_frame,
};

/// Rows requested per `Query`.
pub const PAGE_SIZE: u32 = 50;
/// Largest `limit` spoold serves in one page (it caps longer requests).
pub const MAX_PAGE: u32 = 499;
/// Edits remembered for matching their (failure-only) answers.
const EDITS_REMEMBERED: usize = 256;

/// What the UI thread receives.
#[derive(Debug)]
pub enum Inbound {
  Evt(PickerEvt),
  /// The daemon closed the channel (or sent garbage): exit.
  Closed(String),
}

pub struct Channel {
  tx: mpsc::Sender<PickerReq>,
}

impl Channel {
  /// Handshake on `fd`, then spawn the I/O threads.
  pub fn connect(fd: OwnedFd) -> anyhow::Result<(Channel, cchan::Channel<Inbound>)> {
    let stream = UnixStream::from(fd);
    let mut w = stream.try_clone().context("clone picker socket")?;
    write_frame(&mut w, &Hello::picker()).context("send Hello")?;
    let mut r = BufReader::new(stream);
    let hello: Hello = read_frame(&mut r).context("read daemon Hello")?;
    if !hello.is_picker_compatible() {
      bail!("daemon speaks picker protocol {}, we speak {}", hello.proto, Hello::picker().proto);
    }

    let (in_tx, in_rx) = cchan::channel();
    std::thread::Builder::new().name("picker-rx".into()).spawn(move || {
      loop {
        match read_frame::<_, PickerEvt>(&mut r) {
          Ok(evt) => {
            if in_tx.send(Inbound::Evt(evt)).is_err() {
              return;
            }
          }
          Err(FrameError::Eof) => {
            let _ = in_tx.send(Inbound::Closed("daemon closed the channel".into()));
            return;
          }
          Err(e) => {
            let _ = in_tx.send(Inbound::Closed(format!("read error: {e}")));
            return;
          }
        }
      }
    })?;

    let (tx, rx) = mpsc::channel::<PickerReq>();
    std::thread::Builder::new().name("picker-tx".into()).spawn(move || {
      let mut w = BufWriter::new(w);
      while let Ok(req) = rx.recv() {
        if write_frame(&mut w, &req).is_err() {
          return;
        }
        // Coalesce whatever else is queued, then flush once.
        while let Ok(more) = rx.try_recv() {
          if write_frame(&mut w, &more).is_err() {
            return;
          }
        }
        if w.flush().is_err() {
          return;
        }
      }
    })?;
    Ok((Channel { tx }, in_rx))
  }

  pub fn send(&self, req: PickerReq) {
    // A dead writer means the daemon is gone; the reader reports that.
    let _ = self.tx.send(req);
  }
}

/// Events after correlation, as the UI wants them.
#[derive(Debug, PartialEq)]
pub enum Event {
  Show {
    cursor: Option<CursorPos>,
    output: Option<String>,
    scale_hint: Option<f64>,
  },
  Hide,
  /// A page for the *current* query. `offset == 0` replaces the list.
  Page {
    offset: u32,
    items: Vec<ItemPreview>,
    more: bool,
  },
  /// Thumbnail for `id` (empty = unavailable).
  Thumb {
    id: WireItemId,
    bytes: Vec<u8>,
  },
  /// The daemon could not produce the thumbnail.
  ThumbFailed {
    id: WireItemId,
  },
  /// A `Pin`/`Delete`/`Tag` for item `id` failed.
  EditFailed {
    id: WireItemId,
    code: PickerErrorCode,
    message: String,
  },
  /// The current query failed (bad syntax, ...).
  QueryFailed {
    code: PickerErrorCode,
    message: String,
  },
  /// An error not tied to a live request.
  Notice {
    code: PickerErrorCode,
    message: String,
  },
  NewItem(ItemPreview),
  Locked(Vec<UnlockPrompt>),
  Unlocked,
  UnlockFailed {
    provider: UnlockProvider,
    reason: UnlockFailReason,
  },
  IndexProgress {
    done: u64,
    total: u64,
  },
  /// Answer to an outdated query or an unknown `seq`; ignore.
  Stale,
}

#[derive(Debug, Clone, Copy)]
struct PendingQuery {
  seq: u32,
  offset: u32,
}

/// Request numbering and answer matching (picker protocol v3).
#[derive(Debug, Default)]
pub struct Correlator {
  next_seq: u32,
  query: String,
  /// The latest query, while unanswered. Anything else is stale.
  latest: Option<PendingQuery>,
  thumbs: HashMap<u32, WireItemId>,
  /// Recent edits (`seq`, item). Successes are never answered, so this is
  /// a bounded window rather than a set of pending requests.
  edits: VecDeque<(u32, WireItemId)>,
}

impl Correlator {
  fn seq(&mut self) -> u32 {
    self.next_seq = self.next_seq.wrapping_add(1);
    self.next_seq
  }

  /// Start a new search (`offset` 0). Pages in flight become stale.
  pub fn new_query(&mut self, q: &str) -> PickerReq {
    self.query = q.to_string();
    self.page_req(0)
  }

  /// Next page of the current search (only after the previous page landed).
  pub fn more(&mut self, offset: u32) -> PickerReq {
    self.page_req(offset)
  }

  /// The current search again from the top, `limit` rows (capped at
  /// [`MAX_PAGE`]): a reload after a failed edit. Pages in flight become
  /// stale.
  pub fn reload(&mut self, limit: u32) -> PickerReq {
    let mut req = self.page_req(0);
    if let PickerReq::Query { limit: l, .. } = &mut req {
      *l = limit.clamp(1, MAX_PAGE);
    }
    req
  }

  fn edit_seq(&mut self, id: WireItemId) -> u32 {
    let seq = self.seq();
    if self.edits.len() == EDITS_REMEMBERED {
      self.edits.pop_front();
    }
    self.edits.push_back((seq, id));
    seq
  }

  pub fn pin(&mut self, id: WireItemId, on: bool) -> PickerReq {
    PickerReq::Pin { id, on, seq: self.edit_seq(id) }
  }

  pub fn delete(&mut self, id: WireItemId) -> PickerReq {
    PickerReq::Delete { id, seq: self.edit_seq(id) }
  }

  pub fn tag(&mut self, id: WireItemId, tag: String, on: bool) -> PickerReq {
    PickerReq::Tag { id, tag, on, seq: self.edit_seq(id) }
  }

  fn take_edit(&mut self, seq: u32) -> Option<WireItemId> {
    let i = self.edits.iter().position(|(s, _)| *s == seq)?;
    self.edits.remove(i).map(|(_, id)| id)
  }

  /// Whether the latest query is still unanswered.
  pub fn query_in_flight(&self) -> bool {
    self.latest.is_some()
  }

  fn page_req(&mut self, offset: u32) -> PickerReq {
    let seq = self.seq();
    self.latest = Some(PendingQuery { seq, offset });
    PickerReq::Query {
      seq,
      q: self.query.clone(),
      filters: QueryFilters::default(),
      offset,
      limit: PAGE_SIZE,
    }
  }

  pub fn thumb(&mut self, id: WireItemId, mime: &str) -> PickerReq {
    let seq = self.seq();
    self.thumbs.insert(seq, id);
    PickerReq::Thumb { seq, id, mime: mime.to_string() }
  }

  pub fn thumbs_in_flight(&self) -> usize {
    self.thumbs.len()
  }

  fn take_latest(&mut self, seq: u32) -> Option<PendingQuery> {
    match self.latest {
      Some(p) if p.seq == seq => self.latest.take(),
      _ => None,
    }
  }

  pub fn on_event(&mut self, evt: PickerEvt) -> Event {
    match evt {
      PickerEvt::Show { cursor, output, target_window: _, scale_hint } => {
        Event::Show { cursor, output, scale_hint }
      }
      PickerEvt::Hide => Event::Hide,
      PickerEvt::Page { seq, offset, items, more } => match self.take_latest(seq) {
        // Trust our own record of the offset over the echo.
        Some(p) if p.offset == offset => Event::Page { offset: p.offset, items, more },
        _ => Event::Stale,
      },
      PickerEvt::Thumb { seq, id, bytes } => match self.thumbs.remove(&seq) {
        Some(want) if want == id => Event::Thumb { id, bytes },
        Some(want) => Event::ThumbFailed { id: want },
        None => Event::Stale,
      },
      PickerEvt::Error { seq: Some(seq), code, message } => {
        if self.take_latest(seq).is_some() {
          Event::QueryFailed { code, message }
        } else if let Some(id) = self.thumbs.remove(&seq) {
          Event::ThumbFailed { id }
        } else if let Some(id) = self.take_edit(seq) {
          Event::EditFailed { id, code, message }
        } else {
          Event::Stale
        }
      }
      PickerEvt::Error { seq: None, code, message } => Event::Notice { code, message },
      PickerEvt::NewItem { preview } => Event::NewItem(preview),
      PickerEvt::Locked { providers } => Event::Locked(providers),
      PickerEvt::Unlocked => Event::Unlocked,
      PickerEvt::UnlockFailed { provider, reason } => Event::UnlockFailed { provider, reason },
      PickerEvt::IndexProgress { done, total } => Event::IndexProgress { done, total },
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use spool_proto::PreviewKind;

  fn item(id: i64) -> ItemPreview {
    ItemPreview {
      id,
      kind: PreviewKind::Text,
      preview: format!("item {id}"),
      mimes: vec!["text/plain".into()],
      source_app: None,
      created_unix_ms: 0,
      last_used_unix_ms: 0,
      pinned: false,
      tags: vec![],
      total_size: 1,
    }
  }

  fn seq_of(r: &PickerReq) -> u32 {
    match r {
      PickerReq::Query { seq, .. } | PickerReq::Thumb { seq, .. } => *seq,
      other => panic!("{other:?}"),
    }
  }

  fn page(seq: u32, offset: u32, items: Vec<ItemPreview>, more: bool) -> PickerEvt {
    PickerEvt::Page { seq, offset, items, more }
  }

  #[test]
  fn pages_match_by_seq_and_drop_stale() {
    let mut a = Correlator::default();
    let r1 = a.new_query("a");
    assert!(matches!(r1, PickerReq::Query { ref q, offset: 0, limit: PAGE_SIZE, .. } if q == "a"));
    let r2 = a.new_query("ab");
    assert!(matches!(r2, PickerReq::Query { ref q, offset: 0, .. } if q == "ab"));
    let (s1, s2) = (seq_of(&r1), seq_of(&r2));
    assert!(s2 > s1);
    assert!(a.query_in_flight());
    // Out of order: the answer to "ab" first, then the late one for "a".
    assert_eq!(
      a.on_event(page(s2, 0, vec![item(2)], false)),
      Event::Page { offset: 0, items: vec![item(2)], more: false }
    );
    assert_eq!(a.on_event(page(s1, 0, vec![item(1)], true)), Event::Stale);
    assert!(!a.query_in_flight());
    // Duplicate answer: stale.
    assert_eq!(a.on_event(page(s2, 0, vec![item(2)], false)), Event::Stale);
    // Pagination keeps the query string; `more` comes from the daemon.
    let r3 = a.more(50);
    assert!(matches!(r3, PickerReq::Query { ref q, offset: 50, .. } if q == "ab"));
    let full: Vec<_> = (0..PAGE_SIZE as i64).map(item).collect();
    assert_eq!(
      a.on_event(page(seq_of(&r3), 50, full.clone(), true)),
      Event::Page { offset: 50, items: full, more: true }
    );
    // Unsolicited / future seq.
    assert_eq!(a.on_event(page(999, 0, vec![], false)), Event::Stale);
  }

  #[test]
  fn a_page_for_a_superseded_more_request_is_stale() {
    let mut a = Correlator::default();
    let r1 = a.new_query("");
    a.on_event(page(seq_of(&r1), 0, vec![item(1)], true));
    let more = a.more(1);
    let fresh = a.new_query("x");
    assert_eq!(a.on_event(page(seq_of(&more), 1, vec![item(2)], false)), Event::Stale);
    // Wrong offset echo for the right seq is rejected too.
    assert_eq!(a.on_event(page(seq_of(&fresh), 7, vec![], false)), Event::Stale);
  }

  #[test]
  fn thumbs_match_by_seq_in_any_order() {
    let mut a = Correlator::default();
    let t7 = a.thumb(7, "image/png");
    assert!(matches!(t7, PickerReq::Thumb { id: 7, ref mime, .. } if mime == "image/png"));
    let t9 = a.thumb(9, "image/jpeg");
    assert_eq!(a.thumbs_in_flight(), 2);
    assert_eq!(
      a.on_event(PickerEvt::Thumb { seq: seq_of(&t9), id: 9, bytes: vec![] }),
      Event::Thumb { id: 9, bytes: vec![] }
    );
    assert_eq!(
      a.on_event(PickerEvt::Thumb { seq: seq_of(&t7), id: 7, bytes: vec![1] }),
      Event::Thumb { id: 7, bytes: vec![1] }
    );
    assert_eq!(
      a.on_event(PickerEvt::Thumb { seq: seq_of(&t7), id: 7, bytes: vec![] }),
      Event::Stale
    );
    // A thumb answer naming another id fails the requested one.
    let t3 = a.thumb(3, "image/png");
    assert_eq!(
      a.on_event(PickerEvt::Thumb { seq: seq_of(&t3), id: 4, bytes: vec![1] }),
      Event::ThumbFailed { id: 3 }
    );
    assert_eq!(a.thumbs_in_flight(), 0);
  }

  #[test]
  fn errors_route_by_seq() {
    let mut a = Correlator::default();
    let old = a.new_query("a");
    let q = a.new_query("a\"");
    let t = a.thumb(5, "image/png");
    let err = |seq| PickerEvt::Error {
      seq: Some(seq),
      code: PickerErrorCode::BadQuery,
      message: "unbalanced quote".into(),
    };
    assert_eq!(a.on_event(err(seq_of(&old))), Event::Stale);
    assert_eq!(
      a.on_event(err(seq_of(&q))),
      Event::QueryFailed { code: PickerErrorCode::BadQuery, message: "unbalanced quote".into() }
    );
    assert!(!a.query_in_flight());
    assert_eq!(a.on_event(err(seq_of(&t))), Event::ThumbFailed { id: 5 });
    assert_eq!(
      a.on_event(PickerEvt::Error {
        seq: None,
        code: PickerErrorCode::Unavailable,
        message: "m".into()
      }),
      Event::Notice { code: PickerErrorCode::Unavailable, message: "m".into() }
    );
  }

  #[test]
  fn edit_errors_name_their_item_and_others_are_notices() {
    let mut a = Correlator::default();
    let pin = a.pin(1, true);
    let del = a.delete(2);
    let tag = a.tag(3, "work".into(), false);
    let q = a.new_query("");
    let seq = |r: &PickerReq| match r {
      PickerReq::Pin { seq, .. } | PickerReq::Delete { seq, .. } | PickerReq::Tag { seq, .. } => {
        *seq
      }
      other => seq_of(other),
    };
    assert!(matches!(tag, PickerReq::Tag { id: 3, ref tag, on: false, .. } if tag == "work"));
    // One counter for everything.
    let all = [seq(&pin), seq(&del), seq(&tag), seq(&q)];
    assert!(all.windows(2).all(|w| w[0] < w[1]), "{all:?}");
    let err = |seq| PickerEvt::Error {
      seq: Some(seq),
      code: PickerErrorCode::NotFound,
      message: "gone".into(),
    };
    assert_eq!(
      a.on_event(err(seq(&del))),
      Event::EditFailed { id: 2, code: PickerErrorCode::NotFound, message: "gone".into() }
    );
    assert_eq!(a.on_event(err(seq(&del))), Event::Stale, "answered once");
    assert!(matches!(a.on_event(err(seq(&tag))), Event::EditFailed { id: 3, .. }));
    assert!(matches!(a.on_event(err(seq(&pin))), Event::EditFailed { id: 1, .. }));
    // The query is still the live one; an error without seq (e.g. a failed
    // select) is a plain notice, never an edit failure.
    assert!(a.query_in_flight());
    assert!(matches!(
      a.on_event(PickerEvt::Error {
        seq: None,
        code: PickerErrorCode::Unavailable,
        message: "m".into()
      }),
      Event::Notice { .. }
    ));
    // The window of remembered edits is bounded.
    let first = a.pin(9, true);
    for _ in 0..EDITS_REMEMBERED {
      a.pin(10, true);
    }
    assert_eq!(a.edits.len(), EDITS_REMEMBERED);
    assert_eq!(a.on_event(err(seq(&first))), Event::Stale);
  }

  #[test]
  fn reload_keeps_the_query_and_caps_the_limit() {
    let mut a = Correlator::default();
    let q = a.new_query("abc");
    let r = a.reload(120);
    assert!(matches!(r, PickerReq::Query { ref q, offset: 0, limit: 120, .. } if q == "abc"));
    // The reload supersedes the earlier query.
    assert_eq!(a.on_event(page(seq_of(&q), 0, vec![item(1)], false)), Event::Stale);
    assert!(matches!(a.on_event(page(seq_of(&r), 0, vec![item(1)], true)), Event::Page { .. }));
    assert!(matches!(a.reload(10_000), PickerReq::Query { limit: MAX_PAGE, .. }));
  }

  #[test]
  fn passthrough_events() {
    let mut a = Correlator::default();
    assert_eq!(a.on_event(PickerEvt::Hide), Event::Hide);
    assert_eq!(a.on_event(PickerEvt::Unlocked), Event::Unlocked);
    assert_eq!(
      a.on_event(PickerEvt::Locked { providers: vec![UnlockPrompt::Passphrase] }),
      Event::Locked(vec![UnlockPrompt::Passphrase])
    );
    assert_eq!(
      a.on_event(PickerEvt::UnlockFailed {
        provider: UnlockProvider::Fido2,
        reason: UnlockFailReason::PinInvalid
      }),
      Event::UnlockFailed { provider: UnlockProvider::Fido2, reason: UnlockFailReason::PinInvalid }
    );
    assert_eq!(
      a.on_event(PickerEvt::IndexProgress { done: 1, total: 2 }),
      Event::IndexProgress { done: 1, total: 2 }
    );
    assert_eq!(a.on_event(PickerEvt::NewItem { preview: item(3) }), Event::NewItem(item(3)));
    assert_eq!(
      a.on_event(PickerEvt::Show {
        cursor: Some(CursorPos { x: 1, y: 2 }),
        output: Some("DP-1".into()),
        target_window: None,
        scale_hint: Some(1.5),
      }),
      Event::Show {
        cursor: Some(CursorPos { x: 1, y: 2 }),
        output: Some("DP-1".into()),
        scale_hint: Some(1.5)
      }
    );
  }

  #[test]
  fn handshake_and_round_trip_over_socketpair() {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let daemon = std::thread::spawn(move || {
      let mut s = theirs;
      let h: Hello = read_frame(&mut s).unwrap();
      assert!(h.is_picker_compatible());
      write_frame(&mut s, &Hello::picker()).unwrap();
      let req: PickerReq = read_frame(&mut s).unwrap();
      assert_eq!(req, PickerReq::Delete { id: 5, seq: 1 });
      write_frame(&mut s, &PickerEvt::Hide).unwrap();
      // Close -> picker sees Closed.
    });
    let (chan, rx) = Channel::connect(OwnedFd::from(ours)).unwrap();
    chan.send(PickerReq::Delete { id: 5, seq: 1 });
    daemon.join().unwrap();
    let mut got = Vec::new();
    // calloop channels implement a blocking-free try_recv via the inner mpsc.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut el =
      smithay_client_toolkit::reexports::calloop::EventLoop::<Vec<String>>::try_new().unwrap();
    el.handle()
      .insert_source(rx, |e, _, out: &mut Vec<String>| {
        if let cchan::Event::Msg(m) = e {
          out.push(match m {
            Inbound::Evt(PickerEvt::Hide) => "hide".into(),
            Inbound::Evt(other) => format!("{other:?}"),
            Inbound::Closed(_) => "closed".into(),
          });
        }
      })
      .unwrap();
    while got.len() < 2 && std::time::Instant::now() < deadline {
      el.dispatch(std::time::Duration::from_millis(50), &mut got).unwrap();
    }
    assert_eq!(got, vec!["hide".to_string(), "closed".to_string()]);
  }

  #[test]
  fn handshake_rejects_version_mismatch() {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let daemon = std::thread::spawn(move || {
      let mut s = theirs;
      let _: Hello = read_frame(&mut s).unwrap();
      write_frame(&mut s, &Hello { proto: 999 }).unwrap();
    });
    let err = Channel::connect(OwnedFd::from(ours)).err().expect("mismatch must fail");
    assert!(err.to_string().contains("999"));
    daemon.join().unwrap();
  }
}
