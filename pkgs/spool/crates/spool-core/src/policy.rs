//! Ingest policy: decides what gets fetched from the source app and what gets
//! stored.
//!
//! Two phases, because data must be fetched from the source app (over a
//! Wayland pipe) between them:
//!
//! 1. [`Policy::plan`] looks only at the offer metadata (mimes, selection,
//!    source app, pause state) and returns a [`FetchPlan`].
//!    * [`FetchPlan::CheckHintFirst`]: the orchestrator fetches only
//!      [`PASSWORD_MANAGER_HINT`], calls [`Policy::hint_is_secret`]; if
//!      `true` it records a drop ([`PolicyState::record_dropped`]) and stops,
//!      otherwise it calls [`Policy::plan_after_hint`] and continues as for
//!      `Fetch`.
//!    * [`FetchPlan::Fetch`]: the orchestrator fetches exactly `spec.mimes`
//!      with the given caps/timeout and passes the results to phase 2.
//! 2. [`Policy::evaluate`] inspects the fetched bytes (secret patterns,
//!    emptiness, clear detection, size) and returns a [`Decision`]. It builds
//!    the [`NewItem`] (hash, preview, canonical reps + alias reps from
//!    `spec.aliases`).
//!
//! After acting on a decision the orchestrator reports back:
//! [`PolicyState::record_outcome`] after a successful insert (a new item, or
//! a dedupe bump of an existing one), [`PolicyState::record_dropped`]
//! after a `Drop` or `Skip`, [`PolicyState::record_purged`] after a
//! `PurgePrevious`. `PolicyState` uses this to answer
//! [`PolicyState::keepalive_candidate`] and to do clear detection.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use regex::Regex;

use crate::config::Config;
use crate::item::{
  ItemFlags, ItemId, NewItem, Representation, Selection, TEXT_MIMES, dedupe_hash, hash_prefix,
};
use crate::store::InsertOutcome;
use crate::{Error, Result};

/// KDE's "this came from a password manager" marker mime. If offered with
/// content `secret` the item is never stored.
pub const PASSWORD_MANAGER_HINT: &str = "x-kde-passwordManagerHint";

/// Window for clear detection: an empty/whitespace-only selection within this
/// long after the previous stored copy purges that copy.
pub const CLEAR_WINDOW: Duration = Duration::from_secs(60);

/// Offer metadata, available before any data is fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferInfo {
  pub selection: Selection,
  /// Mimes as offered by the source, in offer order. The daemon's own marker
  /// mime is never included (offers marked as ours never reach the policy).
  pub mimes: Vec<String>,
  /// App id of the focused (presumed source) window if known (from the
  /// compositor's focus tracking); `None` without a focus source.
  pub source_app: Option<String>,
  /// When the offer was observed.
  pub at: SystemTime,
}

/// Phase-1 result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchPlan {
  Skip(SkipReason),
  /// Fetch only [`PASSWORD_MANAGER_HINT`] first (small cap, `timeout`).
  CheckHintFirst {
    timeout: Duration,
  },
  Fetch(FetchSpec),
}

/// What to fetch from the source app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchSpec {
  /// Canonical mimes to fetch, each at most once, all offered by the source.
  pub mimes: Vec<String>,
  /// `(alias, canonical)`: offered mimes that are not fetched but stored as
  /// aliases of a fetched canonical mime (e.g. `("UTF8_STRING",
  /// "text/plain;charset=utf-8")`).
  pub aliases: Vec<(String, String)>,
  pub per_rep_cap: usize,
  pub total_cap: usize,
  pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
  Paused,
  PrimaryDisabled,
  ExcludedApp,
  NothingAllowlisted,
}

/// Phase-2 result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
  Store(NewItem),
  Drop(DropReason),
  /// The selection was cleared (empty/whitespace-only) shortly after
  /// `previous` was stored: delete `previous` and store nothing.
  PurgePrevious {
    previous: ItemId,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
  /// Paused between plan and evaluate.
  Paused,
  /// `x-kde-passwordManagerHint: secret`.
  PasswordManagerHint,
  /// Text matched a secret pattern.
  Secret(SecretKind),
  /// Nothing (or only whitespace) was fetched and there is nothing to purge.
  Empty,
  /// Total fetched size exceeds the cap.
  TooLarge,
  /// None of the fetched representations is usable (all failed/oversize).
  NoUsableData,
}

/// Which secret pattern matched (for logging; never log the match itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
  PemPrivateKey,
  GithubToken,
  AwsAccessKey,
  OpenAiStyleKey,
  Jwt,
  SlackToken,
  AgeSecretKey,
  OtpCode,
}

/// Pause setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseUntil {
  Indefinite,
  Until(SystemTime),
}

/// What happened last on a selection (for keep-alive and clear detection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastEvent {
  /// `purgeable`: clear detection may delete `id`, i.e. this run of copies
  /// created it (it did not exist in the history before).
  Stored {
    id: ItemId,
    at: SystemTime,
    purgeable: bool,
  },
  Dropped,
  Purged,
}

/// Mutable policy state, owned by the orchestrator (single task; not shared).
#[derive(Debug, Clone, Default)]
pub struct PolicyState {
  pause: Option<PauseUntil>,
  last: HashMap<Selection, LastEvent>,
}

impl PolicyState {
  pub fn new() -> Self {
    Self::default()
  }

  /// Pause recording. `None` = indefinitely.
  pub fn pause(&mut self, now: SystemTime, duration: Option<Duration>) {
    self.pause = Some(match duration {
      None => PauseUntil::Indefinite,
      Some(d) => PauseUntil::Until(now + d),
    });
  }

  pub fn resume(&mut self) {
    self.pause = None;
  }

  /// Current pause setting, expiring a timed pause that has elapsed.
  pub fn pause_state(&mut self, now: SystemTime) -> Option<PauseUntil> {
    if let Some(PauseUntil::Until(t)) = self.pause
      && now >= t
    {
      self.pause = None;
    }
    self.pause
  }

  pub fn is_paused(&mut self, now: SystemTime) -> bool {
    self.pause_state(now).is_some()
  }

  /// Record that `id` was stored as a new item for `selection`.
  pub fn record_stored(&mut self, selection: Selection, id: ItemId, at: SystemTime) {
    self.last.insert(selection, LastEvent::Stored { id, at, purgeable: true });
  }

  /// Record that a copy on `selection` dedupe-bumped the existing item `id`.
  /// It stays a keep-alive candidate, but clear detection purges it only if
  /// the previous event was the purgeable store of the same item (copied
  /// twice in a row): a clear must never delete history from before.
  pub fn record_bumped(&mut self, selection: Selection, id: ItemId, at: SystemTime) {
    let purgeable = matches!(
      self.last.get(&selection),
      Some(LastEvent::Stored { id: prev, purgeable: true, .. }) if *prev == id
    );
    self.last.insert(selection, LastEvent::Stored { id, at, purgeable });
  }

  /// [`PolicyState::record_stored`] or [`PolicyState::record_bumped`] by
  /// `outcome`.
  pub fn record_outcome(&mut self, selection: Selection, outcome: InsertOutcome, at: SystemTime) {
    match outcome {
      InsertOutcome::Inserted(id) => self.record_stored(selection, id, at),
      InsertOutcome::Bumped(id) => self.record_bumped(selection, id, at),
    }
  }

  /// Record that the latest offer on `selection` was skipped or dropped.
  pub fn record_dropped(&mut self, selection: Selection) {
    self.last.insert(selection, LastEvent::Dropped);
  }

  /// Record that the previous item on `selection` was purged (clear).
  pub fn record_purged(&mut self, selection: Selection) {
    self.last.insert(selection, LastEvent::Purged);
  }

  /// The item keep-alive may re-publish when `selection` is cleared by the
  /// compositor: the last stored item, unless something was dropped or
  /// purged on that selection since. (The orchestrator must still check
  /// that the item exists and is not `SENSITIVE`.)
  pub fn keepalive_candidate(&self, selection: Selection) -> Option<ItemId> {
    match self.last.get(&selection)? {
      LastEvent::Stored { id, .. } => Some(*id),
      _ => None,
    }
  }

  /// Rewrite the item ids this state refers to (keep-alive and clear
  /// candidates), e.g. after the daemon merged its pre-unlock session store
  /// into the persistent one and every id changed. `map` returns how the
  /// item landed, or `None` if it no longer exists; such a selection is
  /// then treated as dropped (nothing to keep alive or purge). An item that
  /// bumped an existing one is no longer purgeable. Pause and timestamps
  /// are kept.
  pub fn remap_ids(&mut self, mut map: impl FnMut(ItemId) -> Option<InsertOutcome>) {
    for ev in self.last.values_mut() {
      if let LastEvent::Stored { id, at, purgeable } = *ev {
        *ev = match map(id) {
          Some(InsertOutcome::Inserted(id)) => LastEvent::Stored { id, at, purgeable },
          Some(InsertOutcome::Bumped(id)) => LastEvent::Stored { id, at, purgeable: false },
          None => LastEvent::Dropped,
        };
      }
    }
  }

  /// Previous stored item for clear detection: the last event on
  /// `selection` was a purgeable store less than [`CLEAR_WINDOW`] before
  /// `now`.
  pub fn clear_candidate(&self, selection: Selection, now: SystemTime) -> Option<ItemId> {
    match self.last.get(&selection)? {
      LastEvent::Stored { id, at, purgeable: true } => {
        let age = now.duration_since(*at).unwrap_or(Duration::ZERO);
        (age < CLEAR_WINDOW).then_some(*id)
      }
      _ => None,
    }
  }
}

/// Prefix of the daemon's own marker mime (`application/x-spool-source;nonce=…`,
/// see `spool_wayland::MARKER_MIME_PREFIX`). Offers carrying it are ours and
/// should never reach the policy; the policy also refuses to fetch or store
/// any mime starting with this prefix, as defense in depth.
pub const MARKER_MIME_BASE: &str = "application/x-spool-source";

/// Max characters of [`NewItem::preview`].
pub const PREVIEW_CHARS: usize = 200;

/// `true` for our own marker mime (any nonce).
pub fn is_marker_mime(mime: &str) -> bool {
  mime.len() >= MARKER_MIME_BASE.len()
    && mime.as_bytes()[..MARKER_MIME_BASE.len()].eq_ignore_ascii_case(MARKER_MIME_BASE.as_bytes())
}

/// Index into [`TEXT_MIMES`] if `mime` is one of the text variants (ASCII
/// case-insensitive).
fn text_mime_rank(mime: &str) -> Option<usize> {
  TEXT_MIMES.iter().position(|t| t.eq_ignore_ascii_case(mime))
}

/// Text-like content: the [`TEXT_MIMES`] variants and any `text/*`.
fn is_textual(mime: &str) -> bool {
  text_mime_rank(mime).is_some() || mime.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("text/"))
}

// ---- secret patterns -------------------------------------------------------
//
// High-confidence patterns only (no entropy guessing). Token patterns require
// a non-token character (or start of text) before the prefix so that the
// prefix inside a longer identifier does not match.

static RE_PEM: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----").unwrap());
static RE_GITHUB: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{36}\b|github_pat_[A-Za-z0-9_]{50,})").unwrap()
});
static RE_AWS: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").unwrap());
static RE_SK: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"(?:^|[^A-Za-z0-9_-])sk-([A-Za-z0-9_-]{20,})").unwrap());
static RE_JWT: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"(?:^|[^A-Za-z0-9_.-])eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")
    .unwrap()
});
static RE_SLACK: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"\bxox[abposr]-[0-9]+-[0-9A-Za-z-]{10,}").unwrap());
static RE_AGE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"AGE-SECRET-KEY-1[0-9A-Z]{50,}").unwrap());
static RE_OTP: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"^(?:[0-9]{6,8}|[0-9]{3} [0-9]{3})$").unwrap());

/// Extra check for `sk-` keys so kebab-case prose (`sk-learn-compatible-…`)
/// is not flagged: the body must contain a digit and a run of at least 16
/// alphanumerics (random key material), which real keys always have.
fn sk_body_is_key(body: &str) -> bool {
  let has_digit = body.bytes().any(|b| b.is_ascii_digit());
  let longest_run =
    body.split(|c: char| !c.is_ascii_alphanumeric()).map(str::len).max().unwrap_or(0);
  has_digit && longest_run >= 16
}

fn detect_secret_text(text: &str) -> Option<SecretKind> {
  if RE_PEM.is_match(text) {
    return Some(SecretKind::PemPrivateKey);
  }
  if RE_AGE.is_match(text) {
    return Some(SecretKind::AgeSecretKey);
  }
  if RE_GITHUB.is_match(text) {
    return Some(SecretKind::GithubToken);
  }
  if RE_AWS.is_match(text) {
    return Some(SecretKind::AwsAccessKey);
  }
  if RE_SK.captures_iter(text).any(|c| sk_body_is_key(&c[1])) {
    return Some(SecretKind::OpenAiStyleKey);
  }
  if RE_JWT.is_match(text) {
    return Some(SecretKind::Jwt);
  }
  if RE_SLACK.is_match(text) {
    return Some(SecretKind::SlackToken);
  }
  if RE_OTP.is_match(text.trim()) {
    return Some(SecretKind::OtpCode);
  }
  None
}

/// Decode at most the first `max_bytes` of `data` as UTF-8 (lossy), without
/// splitting a character at the cut.
fn decode_prefix(data: &[u8], max_bytes: usize) -> String {
  let slice = &data[..data.len().min(max_bytes)];
  match std::str::from_utf8(slice) {
    Ok(s) => s.to_owned(),
    // Incomplete char at the cut: drop it.
    Err(e) if e.error_len().is_none() => {
      String::from_utf8_lossy(&slice[..e.valid_up_to()]).into_owned()
    }
    Err(_) => String::from_utf8_lossy(slice).into_owned(),
  }
}

/// Single-line sanitized excerpt: control/format chars and whitespace runs
/// become one space; at most [`PREVIEW_CHARS`] chars (ellipsis if cut).
fn text_preview(data: &[u8]) -> String {
  let raw_cut = data.len() > PREVIEW_CHARS * 8;
  let raw = decode_prefix(data, PREVIEW_CHARS * 8);
  let mut out = String::new();
  let mut pending_space = false;
  let mut count = 0;
  let mut truncated = false;
  for c in raw.chars() {
    let blank = c.is_whitespace()
      || c.is_control()
      || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
    if blank {
      pending_space = !out.is_empty();
      continue;
    }
    let needed = if pending_space { 2 } else { 1 };
    if count + needed > PREVIEW_CHARS - 1 {
      truncated = true;
      break;
    }
    if pending_space {
      out.push(' ');
      count += 1;
      pending_space = false;
    }
    out.push(c);
    count += 1;
  }
  if truncated || (raw_cut && !out.is_empty()) {
    out.push('…');
  }
  out
}

fn bracket_preview(mime: &str, len: usize) -> String {
  let mut s = format!("[{mime} {len} bytes]");
  if s.chars().count() > PREVIEW_CHARS {
    s = s.chars().take(PREVIEW_CHARS - 1).collect::<String>() + "…";
  }
  s
}

/// Preview for canonical reps (no aliases), in stored order.
fn build_preview(reps: &[Representation]) -> Option<String> {
  let canon = reps.iter().filter(|r| !r.is_alias());
  let text = canon
    .clone()
    .filter(|r| text_mime_rank(&r.mime).is_some())
    .min_by_key(|r| text_mime_rank(&r.mime))
    .or_else(|| {
      canon.clone().find(|r| is_textual(&r.mime) && !r.mime.eq_ignore_ascii_case("text/html"))
    });
  if let Some(r) = text {
    let p = text_preview(&r.data);
    if !p.is_empty() {
      return Some(p);
    }
  }
  let first = reps.iter().find(|r| !r.is_alias())?;
  Some(bracket_preview(&first.mime, first.data.len()))
}

/// `true` when the content is blank: every rep is zero-length, or textual
/// and whitespace-only.
fn is_blank(reps: &[Representation]) -> bool {
  reps.iter().all(|r| {
    r.data.is_empty() || (is_textual(&r.mime) && String::from_utf8_lossy(&r.data).trim().is_empty())
  })
}

/// One allowlist entry.
#[derive(Debug, Clone)]
enum MimePattern {
  Exact(String),
  /// `type/*` (stores `type`).
  Type(String),
}

impl MimePattern {
  fn parse(p: &str) -> Result<Self> {
    let p = p.trim();
    if p.is_empty() {
      return Err(Error::Policy("empty mime_allowlist entry".into()));
    }
    if let Some(ty) = p.strip_suffix("/*") {
      if ty.is_empty() || ty.contains(['/', '*']) {
        return Err(Error::Policy(format!("bad mime_allowlist wildcard {p:?}")));
      }
      return Ok(MimePattern::Type(ty.to_ascii_lowercase()));
    }
    if p.contains('*') {
      return Err(Error::Policy(format!("bad mime_allowlist entry {p:?} (only type/* allowed)")));
    }
    Ok(MimePattern::Exact(p.to_ascii_lowercase()))
  }

  fn matches(&self, mime: &str) -> bool {
    match self {
      MimePattern::Exact(m) => m.eq_ignore_ascii_case(mime),
      MimePattern::Type(ty) => {
        mime.split_once('/').is_some_and(|(t, sub)| !sub.is_empty() && t.eq_ignore_ascii_case(ty))
      }
    }
  }
}

/// Immutable, compiled policy (regexes, allowlist, caps). Cheap to share by
/// reference; rebuild on config reload.
#[derive(Debug)]
pub struct Policy {
  config: Config,
  hash_key: [u8; 32],
  allowlist: Vec<MimePattern>,
  /// At least one of [`TEXT_MIMES`] is allowlisted.
  text_allowed: bool,
  /// Lowercased `excluded_apps`.
  excluded_apps: Vec<String>,
}

impl Policy {
  /// Compile the policy. `hash_key` is the dedupe key from
  /// `Store::hash_key()`.
  pub fn new(config: Config, hash_key: [u8; 32]) -> Result<Self> {
    let allowlist =
      config.mime_allowlist.iter().map(|p| MimePattern::parse(p)).collect::<Result<Vec<_>>>()?;
    let text_allowed = TEXT_MIMES.iter().any(|t| allowlist.iter().any(|p| p.matches(t)));
    let excluded_apps = config.excluded_apps.iter().map(|a| a.trim().to_lowercase()).collect();
    // Compile the shared regexes now rather than on the first copy.
    for re in [&RE_PEM, &RE_GITHUB, &RE_AWS, &RE_SK, &RE_JWT, &RE_SLACK, &RE_AGE, &RE_OTP] {
      LazyLock::force(re);
    }
    Ok(Self { config, hash_key, allowlist, text_allowed, excluded_apps })
  }

  fn allowlisted(&self, mime: &str) -> bool {
    self.allowlist.iter().any(|p| p.matches(mime))
  }

  fn app_excluded(&self, app: Option<&str>) -> bool {
    app.is_some_and(|a| {
      let a = a.trim();
      self.excluded_apps.iter().any(|e| e.eq_ignore_ascii_case(a))
    })
  }

  /// Phase 1. Order of checks: paused -> primary disabled -> excluded app ->
  /// hint offered (`CheckHintFirst`) -> allowlist/text collapsing (`Fetch`
  /// or `Skip(NothingAllowlisted)`).
  ///
  /// A `Skip` is also recorded in `state` (as by
  /// [`PolicyState::record_dropped`]): the selection now holds something
  /// that was not stored, so keep-alive must not re-publish an older item.
  pub fn plan(&self, state: &mut PolicyState, offer: &OfferInfo) -> FetchPlan {
    self.plan_inner(state, offer, true)
  }

  /// Phase 1 continued after the hint was fetched and found not secret:
  /// like [`Policy::plan`] but ignoring [`PASSWORD_MANAGER_HINT`]; never
  /// returns `CheckHintFirst`.
  pub fn plan_after_hint(&self, state: &mut PolicyState, offer: &OfferInfo) -> FetchPlan {
    self.plan_inner(state, offer, false)
  }

  fn plan_inner(&self, state: &mut PolicyState, offer: &OfferInfo, check_hint: bool) -> FetchPlan {
    let plan = self.plan_decide(state, offer, check_hint);
    if let FetchPlan::Skip(reason) = plan {
      tracing::debug!(selection = offer.selection.as_str(), ?reason, "offer skipped");
      state.record_dropped(offer.selection);
    }
    plan
  }

  fn plan_decide(&self, state: &mut PolicyState, offer: &OfferInfo, check_hint: bool) -> FetchPlan {
    if state.is_paused(offer.at) {
      return FetchPlan::Skip(SkipReason::Paused);
    }
    if offer.selection == Selection::Primary && !self.config.primary_selection {
      return FetchPlan::Skip(SkipReason::PrimaryDisabled);
    }
    if self.app_excluded(offer.source_app.as_deref()) {
      return FetchPlan::Skip(SkipReason::ExcludedApp);
    }
    if check_hint && offer.mimes.iter().any(|m| m == PASSWORD_MANAGER_HINT) {
      return FetchPlan::CheckHintFirst { timeout: self.config.fetch_timeout() };
    }
    match self.fetch_spec(&offer.mimes) {
      Some(spec) => FetchPlan::Fetch(spec),
      None => FetchPlan::Skip(SkipReason::NothingAllowlisted),
    }
  }

  /// Allowlist filtering + text collapsing. `None` if nothing to fetch.
  fn fetch_spec(&self, offered: &[String]) -> Option<FetchSpec> {
    let usable = |m: &String| !m.is_empty() && m != PASSWORD_MANAGER_HINT && !is_marker_mime(m);

    let mut mimes: Vec<String> = Vec::new();
    let mut aliases: Vec<(String, String)> = Vec::new();

    // Text variants: one canonical fetch (best-ranked offered), others alias.
    if self.text_allowed {
      let mut texts: Vec<&String> =
        offered.iter().filter(|m| usable(m) && text_mime_rank(m).is_some()).collect();
      texts.sort_by_key(|m| text_mime_rank(m));
      texts.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
      if let Some((canon, rest)) = texts.split_first() {
        mimes.push((*canon).clone());
        aliases.extend(rest.iter().map(|a| ((*a).clone(), (*canon).clone())));
      }
    }

    // Everything else allowlisted, in offer order, each once.
    for m in offered {
      if !usable(m) || text_mime_rank(m).is_some() || !self.allowlisted(m) {
        continue;
      }
      if mimes.iter().any(|x| x.eq_ignore_ascii_case(m)) {
        continue;
      }
      mimes.push(m.clone());
    }

    if mimes.is_empty() {
      return None;
    }
    Some(FetchSpec {
      mimes,
      aliases,
      per_rep_cap: self.config.max_rep_bytes,
      total_cap: self.config.max_item_bytes,
      timeout: self.config.fetch_timeout(),
    })
  }

  /// `true` if the hint payload (trimmed, ASCII case-insensitive) is
  /// `secret`.
  pub fn hint_is_secret(&self, hint: &[u8]) -> bool {
    hint.trim_ascii().eq_ignore_ascii_case(b"secret")
  }

  /// Phase 2. `reps` are the fetched canonical representations (no
  /// aliases; `alias_of = None`), in `FetchSpec::mimes` order, possibly
  /// missing ones that failed or were over the per-rep cap. `spec` is the
  /// plan that produced them (for aliases and caps).
  ///
  /// Order: paused -> unusable/oversize -> blank (clear detection:
  /// `PurgePrevious` if [`PolicyState::clear_candidate`], else
  /// `Drop(Empty)`) -> secret patterns (every textual rep) -> `Store`.
  ///
  /// `Drop` and `PurgePrevious` are also recorded in `state` (as by
  /// [`PolicyState::record_dropped`] / [`PolicyState::record_purged`]), so
  /// keep-alive can never resurrect a secret or a cleared item even if the
  /// orchestrator forgets to report back; reporting again is harmless.
  pub fn evaluate(
    &self,
    state: &mut PolicyState,
    offer: &OfferInfo,
    spec: &FetchSpec,
    reps: Vec<Representation>,
  ) -> Decision {
    let sel = offer.selection;
    let decision = self.evaluate_inner(state, offer, spec, reps);
    match &decision {
      Decision::Store(item) => tracing::debug!(
        selection = sel.as_str(),
        hash = %hash_prefix(&item.hash),
        len = item.total_size(),
        "offer accepted"
      ),
      Decision::Drop(reason) => {
        tracing::debug!(selection = sel.as_str(), ?reason, "offer dropped");
        state.record_dropped(sel);
      }
      Decision::PurgePrevious { previous } => {
        tracing::debug!(selection = sel.as_str(), %previous, "selection cleared; purging previous");
        state.record_purged(sel);
      }
    }
    decision
  }

  fn evaluate_inner(
    &self,
    state: &mut PolicyState,
    offer: &OfferInfo,
    spec: &FetchSpec,
    reps: Vec<Representation>,
  ) -> Decision {
    if state.is_paused(offer.at) {
      return Decision::Drop(DropReason::Paused);
    }
    let per_rep_cap = spec.per_rep_cap.min(self.config.max_rep_bytes);
    let total_cap = spec.total_cap.min(self.config.max_item_bytes);

    // Keep only reps that were asked for, each once, in spec order.
    let mut kept: Vec<Representation> = Vec::with_capacity(spec.mimes.len());
    for want in &spec.mimes {
      if is_marker_mime(want) || want == PASSWORD_MANAGER_HINT {
        continue;
      }
      if let Some(r) = reps.iter().find(|r| &r.mime == want && !r.is_alias())
        && r.data.len() <= per_rep_cap
      {
        kept.push(Representation::new(r.mime.clone(), r.data.clone()));
      }
    }
    if kept.is_empty() {
      return Decision::Drop(DropReason::NoUsableData);
    }
    let total: usize = kept.iter().map(|r| r.data.len()).sum();
    if total > total_cap {
      return Decision::Drop(DropReason::TooLarge);
    }

    if is_blank(&kept) {
      return match state.clear_candidate(offer.selection, offer.at) {
        Some(previous) => Decision::PurgePrevious { previous },
        None => Decision::Drop(DropReason::Empty),
      };
    }

    if let Some(kind) = self.scan_reps(&kept) {
      return Decision::Drop(DropReason::Secret(kind));
    }

    // Aliases whose canonical survived.
    for (alias, canon) in &spec.aliases {
      if kept.iter().any(|r| &r.mime == canon)
        && !kept.iter().any(|r| &r.mime == alias)
        && !is_marker_mime(alias)
        && alias != PASSWORD_MANAGER_HINT
      {
        kept.push(Representation::alias(alias.clone(), canon.clone()));
      }
    }

    Decision::Store(self.build_item(offer.selection, offer.source_app.clone(), offer.at, kept))
  }

  fn scan_reps(&self, reps: &[Representation]) -> Option<SecretKind> {
    reps
      .iter()
      .filter(|r| !r.is_alias() && is_textual(&r.mime))
      .find_map(|r| detect_secret_text(&String::from_utf8_lossy(&r.data)))
  }

  /// `reps`: canonical first, aliases after; non-empty.
  fn build_item(
    &self,
    selection: Selection,
    source_app: Option<String>,
    at: SystemTime,
    reps: Vec<Representation>,
  ) -> NewItem {
    let hash = dedupe_hash(&self.hash_key, &reps[0]);
    let preview = build_preview(&reps);
    NewItem {
      selection,
      source_app,
      created_at: at,
      flags: ItemFlags::empty(),
      hash,
      preview,
      reps,
    }
  }

  /// Build a [`NewItem`] for data the user explicitly copied via
  /// `spoolctl copy` (`PublicReq::Copy`). Applies size caps and hashing.
  /// Adds text aliases (the other [`TEXT_MIMES`]) when `mime` is a text
  /// mime. The mime allowlist is not applied (the user asked for it).
  ///
  /// Secret patterns **are** applied to textual mimes: a secret yields
  /// `Err(DropReason::Secret(_))` and must not be stored. (Whether the
  /// daemon still publishes it to the clipboard is its call; if it does,
  /// it must `record_dropped` so keep-alive never re-publishes it.)
  ///
  /// Errors: zero-length data -> `Empty`; over `max_rep_bytes` or
  /// `max_item_bytes` -> `TooLarge`; empty mime, our marker mime or
  /// [`PASSWORD_MANAGER_HINT`] -> `NoUsableData`. Whitespace-only text is
  /// accepted (explicit user action; no clear detection here).
  pub fn manual_item(
    &self,
    selection: Selection,
    mime: &str,
    data: Bytes,
    at: SystemTime,
  ) -> std::result::Result<NewItem, DropReason> {
    self.manual_reps(selection, vec![(mime.to_owned(), data)], at)
  }

  /// [`Policy::manual_item`] for several representations of one explicit
  /// user action (an edit saved from an external editor: e.g. the edited
  /// `text/html` plus the plain text derived from it). The first entry is
  /// canonical (its bytes are hashed). Every entry gets `manual_item`'s
  /// checks (mime, empty, per-representation cap, secret patterns on
  /// textual mimes); the sum must fit `max_item_bytes`. The first entry that
  /// is one of [`TEXT_MIMES`] gets the other text variants as aliases.
  /// Duplicate mimes keep the first. Same errors as `manual_item`.
  pub fn manual_reps(
    &self,
    selection: Selection,
    reps: Vec<(String, Bytes)>,
    at: SystemTime,
  ) -> std::result::Result<NewItem, DropReason> {
    if reps.is_empty() {
      return Err(DropReason::NoUsableData);
    }
    let mut canon: Vec<Representation> = Vec::with_capacity(reps.len());
    let mut total = 0usize;
    for (mime, data) in reps {
      let mime = mime.trim();
      if mime.is_empty() || is_marker_mime(mime) || mime == PASSWORD_MANAGER_HINT {
        return Err(DropReason::NoUsableData);
      }
      if data.is_empty() {
        return Err(DropReason::Empty);
      }
      total = total.saturating_add(data.len());
      if data.len() > self.config.max_rep_bytes || total > self.config.max_item_bytes {
        return Err(DropReason::TooLarge);
      }
      if is_textual(mime)
        && let Some(kind) = detect_secret_text(&String::from_utf8_lossy(&data))
      {
        tracing::debug!(len = data.len(), mime, ?kind, "manual item not stored: secret");
        return Err(DropReason::Secret(kind));
      }
      if !canon.iter().any(|r| r.mime.eq_ignore_ascii_case(mime)) {
        canon.push(Representation::new(mime, data));
      }
    }
    let mut aliases = Vec::new();
    if let Some(text) = canon.iter().find(|r| text_mime_rank(&r.mime).is_some()) {
      aliases.extend(
        TEXT_MIMES
          .iter()
          .filter(|t| !canon.iter().any(|r| r.mime.eq_ignore_ascii_case(t)))
          .map(|t| Representation::alias(*t, text.mime.clone())),
      );
    }
    canon.extend(aliases);
    Ok(self.build_item(selection, None, at, canon))
  }

  /// Classify text against the secret patterns. Exposed for tests and for
  /// future re-scans.
  pub fn detect_secret(&self, text: &str) -> Option<SecretKind> {
    detect_secret_text(text)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const T0: SystemTime = SystemTime::UNIX_EPOCH;

  #[test]
  fn pause_expires() {
    let mut s = PolicyState::new();
    s.pause(T0, Some(Duration::from_secs(10)));
    assert!(s.is_paused(T0 + Duration::from_secs(9)));
    assert!(!s.is_paused(T0 + Duration::from_secs(10)));
    s.pause(T0, None);
    assert_eq!(s.pause_state(T0 + Duration::from_secs(1_000_000)), Some(PauseUntil::Indefinite));
    s.resume();
    assert!(!s.is_paused(T0));
  }

  #[test]
  fn keepalive_and_clear_tracking() {
    let mut s = PolicyState::new();
    let c = Selection::Clipboard;
    assert_eq!(s.keepalive_candidate(c), None);
    s.record_stored(c, ItemId(3), T0);
    assert_eq!(s.keepalive_candidate(c), Some(ItemId(3)));
    assert_eq!(s.clear_candidate(c, T0 + Duration::from_secs(59)), Some(ItemId(3)));
    assert_eq!(s.clear_candidate(c, T0 + Duration::from_secs(60)), None);
    assert_eq!(s.keepalive_candidate(Selection::Primary), None);
    s.record_dropped(c);
    assert_eq!(s.keepalive_candidate(c), None);
    assert_eq!(s.clear_candidate(c, T0), None);
  }

  #[test]
  fn bump_of_existing_item_is_never_purged() {
    let mut s = PolicyState::new();
    let c = Selection::Clipboard;
    let soon = T0 + Duration::from_secs(1);
    // A copy that matched an item already in the history: keep-alive yes,
    // clear detection no.
    s.record_outcome(c, InsertOutcome::Bumped(ItemId(9)), T0);
    assert_eq!(s.keepalive_candidate(c), Some(ItemId(9)));
    assert_eq!(s.clear_candidate(c, soon), None);
    // Copied once (new), then again (bump of the same item): still ours.
    s.record_outcome(c, InsertOutcome::Inserted(ItemId(10)), T0);
    s.record_outcome(c, InsertOutcome::Bumped(ItemId(10)), T0);
    assert_eq!(s.clear_candidate(c, soon), Some(ItemId(10)));
    // A bump of some other item after that is not.
    s.record_outcome(c, InsertOutcome::Bumped(ItemId(9)), T0);
    assert_eq!(s.clear_candidate(c, soon), None);
  }

  #[test]
  fn remap_bumped_is_not_purgeable() {
    let mut s = PolicyState::new();
    let c = Selection::Clipboard;
    s.record_stored(c, ItemId(3), T0);
    s.remap_ids(|_| Some(InsertOutcome::Bumped(ItemId(30))));
    assert_eq!(s.keepalive_candidate(c), Some(ItemId(30)));
    assert_eq!(s.clear_candidate(c, T0 + Duration::from_secs(1)), None);
  }

  #[test]
  fn remap_ids() {
    let mut s = PolicyState::new();
    let (c, p) = (Selection::Clipboard, Selection::Primary);
    s.record_stored(c, ItemId(3), T0);
    s.record_stored(p, ItemId(4), T0);
    s.pause(T0, None);
    s.remap_ids(|id| (id == ItemId(3)).then_some(InsertOutcome::Inserted(ItemId(30))));
    assert_eq!(s.keepalive_candidate(c), Some(ItemId(30)));
    assert_eq!(s.clear_candidate(c, T0 + Duration::from_secs(1)), Some(ItemId(30)));
    assert_eq!(s.keepalive_candidate(p), None);
    assert!(s.is_paused(T0));
  }
}

#[cfg(test)]
mod policy_tests {
  use super::*;

  const T0: SystemTime = SystemTime::UNIX_EPOCH;
  const KEY: [u8; 32] = [9; 32];

  fn policy() -> Policy {
    Policy::new(Config::default(), KEY).unwrap()
  }

  fn policy_with(f: impl FnOnce(&mut Config)) -> Policy {
    let mut c = Config::default();
    f(&mut c);
    Policy::new(c, KEY).unwrap()
  }

  fn at(secs: u64) -> SystemTime {
    T0 + Duration::from_secs(1_000_000 + secs)
  }

  fn offer(mimes: &[&str], t: SystemTime) -> OfferInfo {
    OfferInfo {
      selection: Selection::Clipboard,
      mimes: mimes.iter().map(|s| s.to_string()).collect(),
      source_app: None,
      at: t,
    }
  }

  const WLCOPY: &[&str] =
    &["text/plain", "text/plain;charset=utf-8", "TEXT", "STRING", "UTF8_STRING"];

  fn spec_of(plan: FetchPlan) -> FetchSpec {
    match plan {
      FetchPlan::Fetch(s) => s,
      other => panic!("expected Fetch, got {other:?}"),
    }
  }

  /// Plan + evaluate a text copy.
  fn copy_text(p: &Policy, st: &mut PolicyState, text: &str, t: SystemTime) -> Decision {
    let o = offer(WLCOPY, t);
    let spec = spec_of(p.plan(st, &o));
    let reps = vec![Representation::new(spec.mimes[0].clone(), text.as_bytes().to_vec())];
    p.evaluate(st, &o, &spec, reps)
  }

  fn stored(d: Decision) -> NewItem {
    match d {
      Decision::Store(i) => i,
      other => panic!("expected Store, got {other:?}"),
    }
  }

  // ---- secret patterns ----------------------------------------------------

  fn detect(s: &str) -> Option<SecretKind> {
    policy().detect_secret(s)
  }

  #[test]
  fn secret_pem() {
    for hdr in [
      "RSA PRIVATE KEY",
      "PRIVATE KEY",
      "OPENSSH PRIVATE KEY",
      "EC PRIVATE KEY",
      "ENCRYPTED PRIVATE KEY",
      "PGP PRIVATE KEY BLOCK",
    ] {
      let s =
        format!("-----BEGIN {hdr}-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END {hdr}-----\n");
      assert_eq!(detect(&s), Some(SecretKind::PemPrivateKey), "{hdr}");
    }
    // Embedded in a larger paste.
    assert_eq!(
      detect("here:\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl\n"),
      Some(SecretKind::PemPrivateKey)
    );
    assert_eq!(detect("-----BEGIN PUBLIC KEY-----\nMIIBIjAN\n-----END PUBLIC KEY-----"), None);
    assert_eq!(detect("-----BEGIN CERTIFICATE-----\nMIID\n-----END CERTIFICATE-----"), None);
    assert_eq!(detect("the private key is stored in the TPM"), None);
  }

  #[test]
  fn secret_github() {
    let body = "a".repeat(30) + "Z12345";
    for pfx in ["ghp", "gho", "ghu", "ghs", "ghr"] {
      assert_eq!(detect(&format!("{pfx}_{body}")), Some(SecretKind::GithubToken), "{pfx}");
      assert_eq!(
        detect(&format!("export GITHUB_TOKEN={pfx}_{body}\n")),
        Some(SecretKind::GithubToken)
      );
    }
    let pat = format!("github_pat_11ABCDEFG0{}_{}", "x".repeat(12), "Y".repeat(59));
    assert_eq!(detect(&pat), Some(SecretKind::GithubToken));
    assert_eq!(detect("ghp_short123"), None);
    assert_eq!(detect(&format!("ghx_{body}")), None);
    assert_eq!(detect(&format!("ghp_{body}extra")), None, "too long for a classic token");
    assert_eq!(detect("see github_pat_ docs for fine-grained tokens"), None);
  }

  #[test]
  fn secret_aws() {
    assert_eq!(detect("AKIAIOSFODNN7EXAMPLE"), Some(SecretKind::AwsAccessKey));
    assert_eq!(
      detect("aws_access_key_id = AKIAIOSFODNN7EXAMPLE\n"),
      Some(SecretKind::AwsAccessKey)
    );
    assert_eq!(detect("AKIAIOSFODNN7EXAMPL"), None, "15 chars");
    assert_eq!(detect("XAKIAIOSFODNN7EXAMPLE"), None);
    assert_eq!(detect("akiaiosfodnn7example"), None);
    assert_eq!(detect("AKIA is the prefix of AWS access keys"), None);
  }

  #[test]
  fn secret_sk_keys() {
    let openai = "sk-proj-Ab3dEfGh1jKlMnOpQrStUvWx_yz0123456789ABCdef";
    assert_eq!(detect(openai), Some(SecretKind::OpenAiStyleKey));
    assert_eq!(
      detect("sk-1234567890abcdefghijklmnopqrstuvwxyzABCDEFGHIJKL"),
      Some(SecretKind::OpenAiStyleKey)
    );
    let anthropic = "sk-ant-api03-AbCdEfGhIjKlMnOpQrSt1234567890_-abcdefghijklmnopqrstu-AA";
    assert_eq!(detect(anthropic), Some(SecretKind::OpenAiStyleKey));
    assert_eq!(detect(&format!("OPENAI_API_KEY=\"{openai}\"")), Some(SecretKind::OpenAiStyleKey));
    // Prose / identifiers containing "sk-" must not match.
    assert_eq!(detect("I use sk-learn for most of my machine learning work."), None);
    assert_eq!(detect("sk-learn-compatible-estimators-are-great-for-pipelines"), None);
    assert_eq!(detect("the task-sk-abcdefghijklmnopqrstuvwxyz0123 thing"), None);
    assert_eq!(detect("risk-assessment-framework-documentation-2024"), None);
    assert_eq!(detect("desk-1234567890abcdefghijklmnop"), None);
    assert_eq!(detect("sk-short1"), None);
  }

  #[test]
  fn secret_jwt() {
    let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
    assert_eq!(detect(jwt), Some(SecretKind::Jwt));
    assert_eq!(detect(&format!("Authorization: Bearer {jwt}")), Some(SecretKind::Jwt));
    assert_eq!(detect("eyJhbGciOiJIUzI1NiJ9"), None, "one segment");
    assert_eq!(detect("version 1.2.3 of eyJ.foo.bar"), None);
    assert_eq!(detect("abc.def.ghi"), None);
  }

  #[test]
  fn secret_slack() {
    // Split so GitHub push protection doesn't flag this fake token.
    assert_eq!(
      detect(concat!("xox", "b-123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx")),
      Some(SecretKind::SlackToken)
    );
    assert_eq!(detect("token: xoxp-1234-5678-9012-abcdef0123456789"), Some(SecretKind::SlackToken));
    assert_eq!(detect("xoxo-gossip-girl-forever-and-ever"), None);
    assert_eq!(detect("hugs and kisses xoxo"), None);
  }

  #[test]
  fn secret_age() {
    let k = "AGE-SECRET-KEY-1QYQSZQGPQYQSZQGPQYQSZQGPQYQSZQGPQYQSZQGPQYQSZQGPQYQSZQGP0L5WDN";
    assert_eq!(detect(k), Some(SecretKind::AgeSecretKey));
    assert_eq!(
      detect(&format!("# created: 2026\n# public key: age1xyz\n{k}\n")),
      Some(SecretKind::AgeSecretKey)
    );
    assert_eq!(detect("AGE-SECRET-KEY-1 is the prefix"), None);
    assert_eq!(
      detect("age1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqs3290gq"),
      None,
      "public key"
    );
  }

  #[test]
  fn secret_otp() {
    for s in ["123456", "1234567", "12345678", " 482913\n", "482 913"] {
      assert_eq!(detect(s), Some(SecretKind::OtpCode), "{s:?}");
    }
    for s in [
      "12345",
      "123456789",
      "Your order 123456 has shipped",
      "call 555123456 later",
      "12345a",
      "123-456",
      "PIN: 123456",
    ] {
      assert_eq!(detect(s), None, "{s:?}");
    }
  }

  #[test]
  fn ordinary_text_and_code_not_secret() {
    for s in [
      "Hello, world!",
      "fn main() { let sk = 3; let task_id = \"abc\"; }",
      "https://example.com/path?query=1&sk=2",
      "The meeting is at 10:30 in room 204.",
      "git commit -m 'fix(sierra): stop baloo from saturating the NVMe'",
      "",
      "   ",
      "BEGIN PRIVATE KEY",
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbcdefghijklmnopqrstuvwxyz0123456789 me@host",
    ] {
      assert_eq!(detect(s), None, "{s:?}");
    }
  }

  #[test]
  fn secret_copy_is_dropped_and_never_kept_alive() {
    let p = policy();
    let mut st = PolicyState::new();
    let a = stored(copy_text(&p, &mut st, "harmless", at(0)));
    st.record_stored(Selection::Clipboard, ItemId(1), at(0));
    assert_eq!(a.preview.as_deref(), Some("harmless"));
    let d = copy_text(&p, &mut st, "AKIAIOSFODNN7EXAMPLE", at(5));
    assert_eq!(d, Decision::Drop(DropReason::Secret(SecretKind::AwsAccessKey)));
    // evaluate recorded the drop itself: keep-alive must not restore item 1
    // over the secret, nor republish anything.
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), None);
    assert_eq!(st.clear_candidate(Selection::Clipboard, at(6)), None);
    // Orchestrator reporting again is harmless.
    st.record_dropped(Selection::Clipboard);
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), None);
  }

  #[test]
  fn secret_in_html_rep_is_detected() {
    let p = policy();
    let mut st = PolicyState::new();
    let o = offer(&["text/html", "text/plain;charset=utf-8"], at(0));
    let spec = spec_of(p.plan(&mut st, &o));
    let reps = vec![
      Representation::new("text/plain;charset=utf-8", &b"see the key"[..]),
      Representation::new("text/html", &b"<pre>AKIAIOSFODNN7EXAMPLE</pre>"[..]),
    ];
    assert_eq!(
      p.evaluate(&mut st, &o, &spec, reps),
      Decision::Drop(DropReason::Secret(SecretKind::AwsAccessKey))
    );
  }

  // ---- password-manager hint ---------------------------------------------

  #[test]
  fn hint_flow() {
    let p = policy();
    let mut st = PolicyState::new();
    let mut mimes = WLCOPY.to_vec();
    mimes.push(PASSWORD_MANAGER_HINT);
    let o = offer(&mimes, at(0));
    assert_eq!(p.plan(&mut st, &o), FetchPlan::CheckHintFirst { timeout: Duration::from_secs(2) });

    for s in [&b"secret"[..], b"SECRET", b" secret\n", b"Secret\0"[..6].as_ref()] {
      assert!(p.hint_is_secret(s), "{s:?}");
    }
    for s in [&b""[..], b"secrets", b"not secret", b"public"] {
      assert!(!p.hint_is_secret(s), "{s:?}");
    }

    let spec = spec_of(p.plan_after_hint(&mut st, &o));
    assert!(!spec.mimes.iter().any(|m| m == PASSWORD_MANAGER_HINT));
    assert!(
      !spec.aliases.iter().any(|(a, c)| a == PASSWORD_MANAGER_HINT || c == PASSWORD_MANAGER_HINT)
    );
    assert_eq!(spec.mimes, vec!["text/plain;charset=utf-8".to_string()]);
  }

  #[test]
  fn hint_only_offer_after_hint_is_nothing_allowlisted() {
    let p = policy();
    let mut st = PolicyState::new();
    st.record_stored(Selection::Clipboard, ItemId(4), at(0));
    let o = offer(&[PASSWORD_MANAGER_HINT], at(1));
    assert!(matches!(p.plan(&mut st, &o), FetchPlan::CheckHintFirst { .. }));
    assert_eq!(p.plan_after_hint(&mut st, &o), FetchPlan::Skip(SkipReason::NothingAllowlisted));
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), None);
  }

  // ---- clear detection ----------------------------------------------------

  #[test]
  fn clear_within_window_purges_previous() {
    let p = policy();
    let mut st = PolicyState::new();
    stored(copy_text(&p, &mut st, "hunter2-ish password", at(0)));
    st.record_stored(Selection::Clipboard, ItemId(7), at(0));
    for blank in ["", " ", "\n\t "] {
      let mut s2 = st.clone();
      assert_eq!(
        copy_text(&p, &mut s2, blank, at(59)),
        Decision::PurgePrevious { previous: ItemId(7) },
        "{blank:?}"
      );
      assert_eq!(s2.keepalive_candidate(Selection::Clipboard), None);
      // A second clear has nothing left to purge.
      assert_eq!(copy_text(&p, &mut s2, blank, at(60)), Decision::Drop(DropReason::Empty));
    }
  }

  #[test]
  fn clear_outside_window_is_just_empty() {
    let p = policy();
    let mut st = PolicyState::new();
    st.record_stored(Selection::Clipboard, ItemId(7), at(0));
    assert_eq!(copy_text(&p, &mut st, "  ", at(60)), Decision::Drop(DropReason::Empty));
    // Superseded by the clear: keep-alive must not bring item 7 back.
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), None);
  }

  #[test]
  fn clear_on_other_selection_does_not_purge() {
    let p = policy_with(|c| c.primary_selection = true);
    let mut st = PolicyState::new();
    st.record_stored(Selection::Clipboard, ItemId(7), at(0));
    let mut o = offer(WLCOPY, at(1));
    o.selection = Selection::Primary;
    let spec = spec_of(p.plan(&mut st, &o));
    let d =
      p.evaluate(&mut st, &o, &spec, vec![Representation::new(spec.mimes[0].clone(), &b""[..])]);
    assert_eq!(d, Decision::Drop(DropReason::Empty));
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), Some(ItemId(7)));
  }

  #[test]
  fn keepalive_survives_normal_store_cycle() {
    let p = policy();
    let mut st = PolicyState::new();
    stored(copy_text(&p, &mut st, "one", at(0)));
    st.record_stored(Selection::Clipboard, ItemId(1), at(0));
    stored(copy_text(&p, &mut st, "two", at(1)));
    st.record_stored(Selection::Clipboard, ItemId(2), at(1));
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), Some(ItemId(2)));
    // A skipped offer (paused) makes keep-alive stand down.
    st.pause(at(2), None);
    assert_eq!(p.plan(&mut st, &offer(WLCOPY, at(2))), FetchPlan::Skip(SkipReason::Paused));
    assert_eq!(st.keepalive_candidate(Selection::Clipboard), None);
  }

  // ---- phase 1 ------------------------------------------------------------

  #[test]
  fn alias_collapsing() {
    let p = policy();
    let mut st = PolicyState::new();
    let o = offer(WLCOPY, at(0));
    let spec = spec_of(p.plan(&mut st, &o));
    assert_eq!(spec.mimes, vec!["text/plain;charset=utf-8".to_string()]);
    let mut aliases = spec.aliases.clone();
    aliases.sort();
    let c = "text/plain;charset=utf-8".to_string();
    assert_eq!(
      aliases,
      vec![
        ("STRING".to_string(), c.clone()),
        ("TEXT".to_string(), c.clone()),
        ("UTF8_STRING".to_string(), c.clone()),
        ("text/plain".to_string(), c.clone()),
      ]
    );
    assert_eq!(spec.per_rep_cap, 8 * 1024 * 1024);
    assert_eq!(spec.total_cap, 16 * 1024 * 1024);

    let item =
      stored(p.evaluate(&mut st, &o, &spec, vec![Representation::new(c.clone(), &b"hi"[..])]));
    assert_eq!(item.reps.len(), 5);
    assert!(!item.reps[0].is_alias());
    assert!(
      item.reps[1..].iter().all(|r| r.alias_of.as_deref() == Some(c.as_str()) && r.data.is_empty())
    );
    assert_eq!(item.total_size(), 2);
  }

  #[test]
  fn text_canonical_falls_back_by_rank() {
    let p = policy();
    let mut st = PolicyState::new();
    let spec = spec_of(p.plan(&mut st, &offer(&["STRING", "UTF8_STRING"], at(0))));
    assert_eq!(spec.mimes, vec!["UTF8_STRING".to_string()]);
    assert_eq!(spec.aliases, vec![("STRING".into(), "UTF8_STRING".into())]);
  }

  #[test]
  fn allowlist_and_marker_filtering() {
    let p = policy();
    let mut st = PolicyState::new();
    let o = offer(
      &[
        "application/x-spool-source;nonce=abc",
        "application/x-foo-internal",
        "image/png",
        "image/png",
        "text/html",
        "chromium/x-web-custom-data",
        "text/plain",
      ],
      at(0),
    );
    let spec = spec_of(p.plan(&mut st, &o));
    assert_eq!(spec.mimes, vec!["text/plain".to_string(), "image/png".into(), "text/html".into()]);
    assert!(spec.aliases.is_empty());

    assert_eq!(
      p.plan(&mut st, &offer(&["application/x-spool-source;nonce=1", "application/x-foo"], at(0))),
      FetchPlan::Skip(SkipReason::NothingAllowlisted)
    );
    assert_eq!(
      p.plan(&mut st, &offer(&[], at(0))),
      FetchPlan::Skip(SkipReason::NothingAllowlisted)
    );

    // Wildcard allowlist; marker never fetched even if a wildcard matches it.
    let p = policy_with(|c| c.mime_allowlist = vec!["application/*".into(), "text/plain".into()]);
    let spec = spec_of(p.plan(
      &mut st,
      &offer(
        &["application/x-spool-source;nonce=1", "application/json", "TEXT", "image/png"],
        at(0),
      ),
    ));
    assert_eq!(spec.mimes, vec!["TEXT".to_string(), "application/json".into()]);
  }

  #[test]
  fn text_disallowed_when_no_text_mime_listed() {
    let p = policy_with(|c| c.mime_allowlist = vec!["image/*".into()]);
    let mut st = PolicyState::new();
    assert_eq!(
      p.plan(&mut st, &offer(WLCOPY, at(0))),
      FetchPlan::Skip(SkipReason::NothingAllowlisted)
    );
  }

  #[test]
  fn bad_allowlist_rejected() {
    for bad in ["", "*", "*/*", "image/p*", "/*"] {
      assert!(
        Policy::new(Config { mime_allowlist: vec![bad.into()], ..Config::default() }, KEY).is_err(),
        "{bad:?}"
      );
    }
  }

  #[test]
  fn primary_disabled_and_enabled() {
    let mut st = PolicyState::new();
    let mut o = offer(WLCOPY, at(0));
    o.selection = Selection::Primary;
    assert_eq!(policy().plan(&mut st, &o), FetchPlan::Skip(SkipReason::PrimaryDisabled));
    let p = policy_with(|c| c.primary_selection = true);
    assert!(matches!(p.plan(&mut st, &o), FetchPlan::Fetch(_)));
  }

  #[test]
  fn paused_plan_and_evaluate() {
    let p = policy();
    let mut st = PolicyState::new();
    st.pause(at(0), Some(Duration::from_secs(30)));
    assert_eq!(p.plan(&mut st, &offer(WLCOPY, at(10))), FetchPlan::Skip(SkipReason::Paused));
    // Paused beats the hint check.
    assert_eq!(
      p.plan(&mut st, &offer(&[PASSWORD_MANAGER_HINT, "text/plain"], at(10))),
      FetchPlan::Skip(SkipReason::Paused)
    );
    let o = offer(WLCOPY, at(31));
    let spec = spec_of(p.plan(&mut st, &o));
    // Paused between plan and evaluate.
    st.pause(at(31), None);
    let d =
      p.evaluate(&mut st, &o, &spec, vec![Representation::new(spec.mimes[0].clone(), &b"x"[..])]);
    assert_eq!(d, Decision::Drop(DropReason::Paused));
  }

  #[test]
  fn excluded_app() {
    let p = policy_with(|c| c.excluded_apps = vec!["org.keepassxc.KeePassXC".into()]);
    let mut st = PolicyState::new();
    let mut o = offer(WLCOPY, at(0));
    o.source_app = Some("org.KeePassXC.keepassxc".into());
    assert_eq!(p.plan(&mut st, &o), FetchPlan::Skip(SkipReason::ExcludedApp));
    o.source_app = Some("org.kde.kate".into());
    assert!(matches!(p.plan(&mut st, &o), FetchPlan::Fetch(_)));
    o.source_app = None;
    assert!(matches!(p.plan(&mut st, &o), FetchPlan::Fetch(_)));
  }

  // ---- phase 2 ------------------------------------------------------------

  #[test]
  fn caps_enforced() {
    let p = policy_with(|c| {
      c.max_rep_bytes = 10;
      c.max_item_bytes = 15;
    });
    let mut st = PolicyState::new();
    let o = offer(&["text/plain", "image/png", "text/html"], at(0));
    let spec = spec_of(p.plan(&mut st, &o));
    assert_eq!((spec.per_rep_cap, spec.total_cap), (10, 15));

    // Oversize rep omitted, remaining stored.
    let d = p.evaluate(
      &mut st,
      &o,
      &spec,
      vec![
        Representation::new("text/plain", &b"hello"[..]),
        Representation::new("image/png", vec![0u8; 11]),
      ],
    );
    let item = stored(d);
    assert_eq!(item.reps.len(), 1);

    // Only rep oversize -> NoUsableData.
    let d = p.evaluate(&mut st, &o, &spec, vec![Representation::new("text/plain", vec![b'a'; 11])]);
    assert_eq!(d, Decision::Drop(DropReason::NoUsableData));

    // Total over cap -> TooLarge.
    let d = p.evaluate(
      &mut st,
      &o,
      &spec,
      vec![
        Representation::new("text/plain", vec![b'a'; 8]),
        Representation::new("image/png", vec![1u8; 8]),
      ],
    );
    assert_eq!(d, Decision::Drop(DropReason::TooLarge));

    // Nothing fetched.
    assert_eq!(p.evaluate(&mut st, &o, &spec, vec![]), Decision::Drop(DropReason::NoUsableData));
  }

  #[test]
  fn evaluate_ignores_unrequested_reps() {
    let p = policy();
    let mut st = PolicyState::new();
    let o = offer(&["image/png"], at(0));
    let spec = spec_of(p.plan(&mut st, &o));
    let d = p.evaluate(
      &mut st,
      &o,
      &spec,
      vec![
        Representation::new("text/plain", &b"AKIAIOSFODNN7EXAMPLE"[..]),
        Representation::new("application/x-spool-source;nonce=1", &b"x"[..]),
      ],
    );
    assert_eq!(d, Decision::Drop(DropReason::NoUsableData));
  }

  #[test]
  fn image_item_preview_and_hash() {
    let p = policy();
    let mut st = PolicyState::new();
    let o = offer(&["image/png"], at(0));
    let spec = spec_of(p.plan(&mut st, &o));
    let item = stored(p.evaluate(
      &mut st,
      &o,
      &spec,
      vec![Representation::new("image/png", vec![7u8; 1234])],
    ));
    assert_eq!(item.preview.as_deref(), Some("[image/png 1234 bytes]"));
    assert_eq!(item.hash, dedupe_hash(&KEY, &Representation::new("image/png", vec![7u8; 1234])));
    assert_eq!(item.total_size(), 1234);
    assert_eq!(item.created_at, at(0));
    assert_eq!(item.flags, ItemFlags::empty());
  }

  #[test]
  fn text_preview_sanitized_and_truncated() {
    let p = policy();
    let mut st = PolicyState::new();
    let item = stored(copy_text(&p, &mut st, "  line one\n\tline\u{202E}two\r\n\n", at(0)));
    assert_eq!(item.preview.as_deref(), Some("line one line two"));

    let long = "word ".repeat(500);
    let item = stored(copy_text(&p, &mut st, &long, at(1)));
    let pv = item.preview.unwrap();
    assert!(pv.chars().count() <= PREVIEW_CHARS);
    assert!(pv.ends_with('…'));
    assert!(!pv.contains('\n'));

    let multibyte = "é".repeat(5000);
    let pv = stored(copy_text(&p, &mut st, &multibyte, at(2))).preview.unwrap();
    assert_eq!(pv.chars().count(), PREVIEW_CHARS);
    assert!(!pv.contains('\u{FFFD}'));
  }

  #[test]
  fn dedupe_hash_stable_per_content() {
    let p = policy();
    let mut st = PolicyState::new();
    let a = stored(copy_text(&p, &mut st, "same", at(0)));
    let b = stored(copy_text(&p, &mut st, "same", at(1)));
    let c = stored(copy_text(&p, &mut st, "different", at(2)));
    assert_eq!(a.hash, b.hash);
    assert_ne!(a.hash, c.hash);
    let other = Policy::new(Config::default(), [1; 32]).unwrap();
    assert_ne!(stored(copy_text(&other, &mut st, "same", at(3))).hash, a.hash);
  }

  // ---- manual_item ----------------------------------------------------------

  #[test]
  fn manual_item_text_gets_aliases() {
    let p = policy();
    let it = p
      .manual_item(Selection::Clipboard, "text/plain", Bytes::from_static(b"hello\nthere"), at(0))
      .unwrap();
    assert_eq!(it.reps[0], Representation::new("text/plain", &b"hello\nthere"[..]));
    let aliases: Vec<_> = it.reps[1..].iter().map(|r| r.mime.as_str()).collect();
    assert_eq!(aliases, vec!["text/plain;charset=utf-8", "UTF8_STRING", "TEXT", "STRING"]);
    assert!(it.reps[1..].iter().all(|r| r.alias_of.as_deref() == Some("text/plain")));
    assert_eq!(it.preview.as_deref(), Some("hello there"));
    assert_eq!(it.source_app, None);
    assert_eq!(it.hash, dedupe_hash(&KEY, &it.reps[0]));

    let img = p
      .manual_item(Selection::Clipboard, "image/png", Bytes::from_static(b"\x89PNG"), at(0))
      .unwrap();
    assert_eq!(img.reps.len(), 1);
    // Allowlist not applied to explicit copies.
    assert!(
      p.manual_item(Selection::Clipboard, "application/x-custom", Bytes::from_static(b"x"), at(0))
        .is_ok()
    );
    // Whitespace is accepted for explicit copies.
    assert!(
      p.manual_item(Selection::Clipboard, "text/plain", Bytes::from_static(b" "), at(0)).is_ok()
    );
  }

  #[test]
  fn manual_reps_for_an_edited_html_item() {
    let p = policy();
    let it = p
      .manual_reps(
        Selection::Clipboard,
        vec![
          ("text/html".into(), Bytes::from_static(b"<b>hi</b> there")),
          ("text/plain;charset=utf-8".into(), Bytes::from_static(b"hi there")),
        ],
        at(0),
      )
      .unwrap();
    // HTML is canonical (hashed); the derived text carries the aliases.
    assert_eq!(it.reps[0].mime, "text/html");
    assert_eq!(it.hash, dedupe_hash(&KEY, &it.reps[0]));
    assert_eq!(it.reps[1], Representation::new("text/plain;charset=utf-8", &b"hi there"[..]));
    let aliases: Vec<_> = it.reps[2..].iter().map(|r| r.mime.as_str()).collect();
    assert_eq!(aliases, vec!["text/plain", "UTF8_STRING", "TEXT", "STRING"]);
    assert!(it.reps[2..].iter().all(|r| r.alias_of.as_deref() == Some("text/plain;charset=utf-8")));
    assert_eq!(it.preview.as_deref(), Some("hi there"));
    assert_eq!(it.total_size(), 15 + 8);

    // A secret in any representation (here only the HTML) is refused.
    let r = p.manual_reps(
      Selection::Clipboard,
      vec![
        ("text/html".into(), Bytes::from_static(b"<i>AKIAIOSFODNN7EXAMPLE</i>")),
        ("text/plain;charset=utf-8".into(), Bytes::from_static(b"x")),
      ],
      at(0),
    );
    assert_eq!(r, Err(DropReason::Secret(SecretKind::AwsAccessKey)));
    // The item cap covers the sum.
    let small = policy_with(|c| {
      c.max_rep_bytes = 4;
      c.max_item_bytes = 6;
    });
    let r = small.manual_reps(
      Selection::Clipboard,
      vec![
        ("text/html".into(), Bytes::from_static(b"abcd")),
        ("text/plain".into(), Bytes::from_static(b"abc")),
      ],
      at(0),
    );
    assert_eq!(r, Err(DropReason::TooLarge));
    assert_eq!(p.manual_reps(Selection::Clipboard, vec![], at(0)), Err(DropReason::NoUsableData));
  }

  #[test]
  fn manual_item_rejections() {
    let p = policy_with(|c| {
      c.max_rep_bytes = 4;
      c.max_item_bytes = 4;
    });
    let m = |mime: &str, d: &'static [u8]| {
      p.manual_item(Selection::Clipboard, mime, Bytes::from_static(d), at(0))
    };
    assert_eq!(m("text/plain", b""), Err(DropReason::Empty));
    assert_eq!(m("text/plain", b"12345"), Err(DropReason::TooLarge));
    assert_eq!(m("application/x-spool-source;nonce=1", b"x"), Err(DropReason::NoUsableData));
    assert_eq!(m(PASSWORD_MANAGER_HINT, b"x"), Err(DropReason::NoUsableData));
    assert_eq!(m("", b"x"), Err(DropReason::NoUsableData));

    let p = policy();
    assert_eq!(
      p.manual_item(
        Selection::Clipboard,
        "text/plain",
        Bytes::from_static(b"AKIAIOSFODNN7EXAMPLE"),
        at(0)
      ),
      Err(DropReason::Secret(SecretKind::AwsAccessKey))
    );
    // Binary mimes are not pattern-scanned.
    assert!(
      p.manual_item(
        Selection::Clipboard,
        "image/png",
        Bytes::from_static(b"AKIAIOSFODNN7EXAMPLE"),
        at(0)
      )
      .is_ok()
    );
  }
}
