//! The Lua state: loading scripts and running the filter chain.
//!
//! Scripts are loaded in file-name order. A script registers hooks while it
//! runs its top level (`spool.filter(fn)`); a script that fails to load is
//! skipped (its hooks are rolled back) and the others still run.
//!
//! Filter chain: every filter in registration order sees the item as the
//! previous ones left it. `spool.drop()` ends the chain; `spool.keep{..}`
//! adds tags and may replace the content; `nil` keeps it unchanged. A
//! filter that errors or returns something else counts as `nil`. When the
//! time budget runs out the whole chain answers "keep unchanged", the same
//! thing spoold does when no answer comes.
//!
//! The standard library is fully loaded except `debug` (mlua's safe set):
//! scripts are the user's own and may use `io` / `os`. Limits exist only so
//! a buggy script cannot wedge the host: an instruction-count hook enforces
//! the deadline and the heap is capped at [`MEMORY_LIMIT`].
//!
//! Errors are logged as kind + `file:line` only; a Lua message may quote
//! clipboard content. `SPOOL_SCRIPTS_DEBUG=1` logs them in full.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use mlua::{AnyUserData, Function, HookTriggers, Lua, Table, UserData, Value, VmState};
use spool_proto::{ScriptItem, Verdict, WireRep, WireSelection};

use crate::api;

/// Heap cap for the Lua state.
pub const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

/// Longest a script's top level may run while loading.
pub const LOAD_BUDGET: Duration = Duration::from_secs(5);

/// The share of spoold's budget the chain may use (the rest covers the
/// round trip, so an answer is not late by a hair).
const BUDGET_SHARE: f64 = 0.8;

/// The hook checks the deadline every this many VM instructions.
const HOOK_EVERY: u32 = 1000;

/// Message of the error the hook raises (matched to tell timeouts apart).
const BUDGET_EXCEEDED: &str = "spool: time budget exceeded";

/// Development override for scripts outside `/nix/store`.
pub const INSECURE_SCRIPTS_ENV: &str = "SPOOL_INSECURE_SCRIPTS_OK";

/// Text representations in preference order, for `item.text`.
const TEXT_PREFERENCE: &[&str] =
  &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];

/// Default mime of `spool.keep{ text = .. }`.
pub const TEXT_MIME: &str = "text/plain;charset=utf-8";

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
  #[error("cannot read {0}: {1}")]
  Io(PathBuf, std::io::Error),
  #[error(
    "{0} is not under /nix/store; a writable script directory would let any of your programs \
     install a clipboard logger (set {INSECURE_SCRIPTS_ENV}=1 for development)"
  )]
  NotInStore(PathBuf),
}

/// A filter's outcome, carried from Lua to Rust as userdata.
#[derive(Debug, Clone)]
pub struct LuaVerdict(pub Verdict);

impl UserData for LuaVerdict {}

/// Per-state bookkeeping shared with the API callbacks (Lua app data).
#[derive(Default)]
pub struct Registry {
  /// Script whose top level is running (registration allowed) or whose
  /// hook is running (for `spool.log`).
  pub current: Option<String>,
  pub loading: bool,
  /// Filters registered so far, in order.
  pub filters: Vec<Filter>,
}

pub struct Filter {
  pub script: String,
  pub func: Function,
}

/// Deadline checked by the instruction hook.
struct Deadline(Rc<Cell<Option<Instant>>>);

pub struct Engine {
  lua: Lua,
  deadline: Rc<Cell<Option<Instant>>>,
  debug: bool,
}

/// What loading a directory did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
  pub loaded: Vec<String>,
  pub failed: Vec<String>,
}

/// The item as the chain has it so far.
struct Current {
  reps: Vec<WireRep>,
  aliases: Vec<(String, String)>,
  tags: Vec<String>,
  rewritten: bool,
  /// Scripts that rewrote the content, in order.
  by: Vec<String>,
  /// Every rewriting filter allowed keeping the original.
  original: bool,
}

impl Engine {
  pub fn new(debug: bool) -> mlua::Result<Self> {
    let lua = Lua::new();
    lua.set_memory_limit(MEMORY_LIMIT)?;
    let deadline = Rc::new(Cell::new(None));
    lua.set_app_data(Deadline(deadline.clone()));
    lua.set_app_data(Registry::default());
    lua.set_hook(HookTriggers::new().every_nth_instruction(HOOK_EVERY), |lua, _| {
      let over = lua
        .app_data_ref::<Deadline>()
        .and_then(|d| d.0.get())
        .is_some_and(|at| Instant::now() >= at);
      if over {
        Err(mlua::Error::RuntimeError(BUDGET_EXCEEDED.into()))
      } else {
        Ok(VmState::Continue)
      }
    })?;
    api::install(&lua)?;
    Ok(Self { lua, deadline, debug })
  }

  /// Load every `*.lua` file in `dir`, in name order. The directory (and
  /// each file, symlinks resolved) must be in `/nix/store` unless
  /// `insecure_ok`.
  pub fn load_dir(&mut self, dir: &Path, insecure_ok: bool) -> Result<LoadReport, LoadError> {
    let canon = std::fs::canonicalize(dir).map_err(|e| LoadError::Io(dir.into(), e))?;
    if !insecure_ok && !in_store(&canon) {
      return Err(LoadError::NotInStore(canon));
    }
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&canon)
      .map_err(|e| LoadError::Io(canon.clone(), e))?
      .filter_map(Result::ok)
      .filter_map(|e| {
        let name = e.file_name().into_string().ok()?;
        name.ends_with(".lua").then(|| (name, e.path()))
      })
      .collect();
    files.sort();
    let mut report = LoadReport::default();
    for (name, path) in files {
      let real = std::fs::canonicalize(&path).map_err(|e| LoadError::Io(path.clone(), e))?;
      if !insecure_ok && !in_store(&real) {
        return Err(LoadError::NotInStore(real));
      }
      if !real.is_file() {
        continue;
      }
      let src = std::fs::read(&real).map_err(|e| LoadError::Io(real.clone(), e))?;
      match self.load_source(&name, &src) {
        Ok(()) => report.loaded.push(name),
        Err(why) => {
          tracing::error!(script = %name, "not loaded: {why}");
          report.failed.push(name);
        }
      }
    }
    Ok(report)
  }

  /// Run one script's top level, registering its hooks. On error its hooks
  /// are rolled back and the (redacted) reason returned.
  pub fn load_source(&mut self, name: &str, src: &[u8]) -> Result<(), String> {
    let before = {
      let mut reg = self.registry_mut();
      reg.current = Some(name.to_owned());
      reg.loading = true;
      reg.filters.len()
    };
    self.deadline.set(Some(Instant::now() + LOAD_BUDGET));
    let result = self.lua.load(src).set_name(format!("={name}")).exec();
    self.deadline.set(None);
    let mut reg = self.registry_mut();
    reg.current = None;
    reg.loading = false;
    match result {
      Ok(()) => {
        tracing::info!(script = %name, filters = reg.filters.len() - before, "loaded");
        Ok(())
      }
      Err(e) => {
        reg.filters.truncate(before);
        Err(describe(&e, self.debug))
      }
    }
  }

  pub fn filter_count(&self) -> usize {
    self.lua.app_data_ref::<Registry>().map_or(0, |r| r.filters.len())
  }

  /// Run the filter chain on `item` within `budget` (spoold's; the chain
  /// uses most of it).
  pub fn filter(&self, item: &ScriptItem, budget: Duration) -> Verdict {
    let deadline = Instant::now() + budget.mul_f64(BUDGET_SHARE);
    let filters: Vec<(String, Function)> = self
      .lua
      .app_data_ref::<Registry>()
      .map(|r| r.filters.iter().map(|f| (f.script.clone(), f.func.clone())).collect())
      .unwrap_or_default();
    let mut cur = Current {
      reps: item.reps.clone(),
      aliases: item.aliases.clone(),
      tags: Vec::new(),
      rewritten: false,
      by: Vec::new(),
      original: true,
    };
    for (script, func) in filters {
      if Instant::now() >= deadline {
        tracing::warn!(script = %script, "filter chain out of time; copy kept unfiltered");
        return Verdict::keep();
      }
      self.registry_mut().current = Some(script.clone());
      self.deadline.set(Some(deadline));
      let result = self.item_table(item, &cur).and_then(|t| func.call::<Value>(t));
      self.deadline.set(None);
      self.registry_mut().current = None;
      match result {
        Ok(Value::Nil) => {}
        Ok(Value::UserData(ud)) => match verdict_of(&ud) {
          Some(Verdict::Drop) => {
            tracing::debug!(script = %script, "filter dropped the copy");
            return Verdict::Drop;
          }
          Some(Verdict::Keep { tags, reps, original, .. }) => {
            cur.tags.extend(tags);
            if let Some(reps) = reps {
              // Redaction wins: one filter saying "no original" is enough.
              cur.original &= original;
              if !cur.by.contains(&script) {
                cur.by.push(script.clone());
              }
              cur.reps = reps;
              cur.aliases.clear();
              cur.rewritten = true;
            }
          }
          None => tracing::warn!(script = %script, "filter returned a userdata; ignored"),
        },
        Ok(other) => tracing::warn!(
          script = %script,
          returned = other.type_name(),
          "filter must return spool.keep{{..}}, spool.drop() or nil; ignored"
        ),
        Err(e) if Instant::now() >= deadline => {
          tracing::warn!(script = %script, "filter out of time ({}); copy kept unfiltered",
            describe(&e, self.debug));
          return Verdict::keep();
        }
        Err(e) => {
          tracing::warn!(script = %script, "filter failed ({}); skipped", describe(&e, self.debug))
        }
      }
    }
    if !cur.rewritten {
      return Verdict::Keep { tags: cur.tags, reps: None, original: true, by: Vec::new() };
    }
    Verdict::Keep { tags: cur.tags, reps: Some(cur.reps), original: cur.original, by: cur.by }
  }

  fn registry_mut(&self) -> mlua::AppDataRefMut<'_, Registry> {
    self.lua.app_data_mut::<Registry>().expect("registry installed in new()")
  }

  /// The `item` table a filter receives.
  fn item_table(&self, item: &ScriptItem, cur: &Current) -> mlua::Result<Table> {
    let lua = &self.lua;
    let t = lua.create_table()?;
    t.set("id", item.id)?;
    t.set(
      "selection",
      match item.selection {
        WireSelection::Clipboard => "clipboard",
        WireSelection::Primary => "primary",
      },
    )?;
    t.set("source_app", item.source_app.clone())?;
    t.set("created_at", item.created_at_ms)?;
    let tags: Vec<&str> = item.tags.iter().chain(&cur.tags).map(String::as_str).collect();
    t.set("tags", lua.create_sequence_from(tags)?)?;
    let mimes =
      cur.reps.iter().map(|r| r.mime.as_str()).chain(cur.aliases.iter().map(|a| a.0.as_str()));
    t.set("mimes", lua.create_sequence_from(mimes)?)?;
    let resolved = Rc::new(resolve_all(&cur.reps, &cur.aliases));
    if let Some(text) = preferred_text(&resolved) {
      t.set("text", lua.create_string(text)?)?;
    }
    let get = lua.create_function(move |lua, (_this, mime): (Value, String)| {
      match resolved.iter().find(|(m, _)| m.eq_ignore_ascii_case(&mime)) {
        Some((_, data)) => Ok(Value::String(lua.create_string(data)?)),
        None => Ok(Value::Nil),
      }
    })?;
    t.set("get", get)?;
    Ok(t)
  }
}

fn in_store(p: &Path) -> bool {
  p.starts_with("/nix/store")
}

fn verdict_of(ud: &AnyUserData) -> Option<Verdict> {
  ud.borrow::<LuaVerdict>().ok().map(|v| v.0.clone())
}

/// Every mime the item answers to with its bytes (aliases resolved).
fn resolve_all(reps: &[WireRep], aliases: &[(String, String)]) -> Vec<(String, Vec<u8>)> {
  let mut out: Vec<(String, Vec<u8>)> =
    reps.iter().map(|r| (r.mime.clone(), r.data.clone())).collect();
  for (alias, canon) in aliases {
    if let Some(r) = reps.iter().find(|r| &r.mime == canon) {
      out.push((alias.clone(), r.data.clone()));
    }
  }
  out
}

fn preferred_text(resolved: &[(String, Vec<u8>)]) -> Option<&[u8]> {
  TEXT_PREFERENCE.iter().find_map(|want| {
    resolved.iter().find(|(m, _)| m.eq_ignore_ascii_case(want)).map(|(_, d)| d.as_slice())
  })
}

/// Kind and `file:line` of a Lua error; never the message itself (it may
/// quote content) unless `debug`.
pub fn describe(e: &mlua::Error, debug: bool) -> String {
  if debug {
    return e.to_string();
  }
  static LOC: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"([^\s:\[\]]+\.lua):(\d+):").expect("static regex"));
  let (kind, msg) = match innermost(e) {
    mlua::Error::SyntaxError { message, .. } => ("syntax error", message.as_str()),
    mlua::Error::RuntimeError(m) if m.contains(BUDGET_EXCEEDED) => {
      ("time budget exceeded", m.as_str())
    }
    mlua::Error::RuntimeError(m) => ("runtime error", m.as_str()),
    mlua::Error::MemoryError(m) => ("out of memory", m.as_str()),
    _ => ("error", ""),
  };
  match LOC.captures(msg) {
    Some(c) => format!("{kind} at {}:{}", &c[1], &c[2]),
    None => kind.to_owned(),
  }
}

fn innermost(e: &mlua::Error) -> &mlua::Error {
  match e {
    mlua::Error::CallbackError { cause, .. } => innermost(cause),
    other => other,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const BUDGET: Duration = Duration::from_millis(250);

  fn engine(scripts: &[(&str, &str)]) -> Engine {
    let mut e = Engine::new(false).unwrap();
    for (name, src) in scripts {
      e.load_source(name, src.as_bytes()).unwrap_or_else(|why| panic!("{name}: {why}"));
    }
    e
  }

  fn text_item(text: &str, app: Option<&str>) -> ScriptItem {
    ScriptItem {
      id: None,
      selection: WireSelection::Clipboard,
      source_app: app.map(str::to_owned),
      created_at_ms: 1_700_000_000_000,
      tags: vec![],
      reps: vec![WireRep { mime: TEXT_MIME.into(), data: text.as_bytes().to_vec() }],
      aliases: vec![("UTF8_STRING".into(), TEXT_MIME.into())],
    }
  }

  fn rewritten_text(v: &Verdict) -> Option<String> {
    match v {
      Verdict::Keep { reps: Some(reps), .. } => {
        Some(String::from_utf8(reps[0].data.clone()).unwrap())
      }
      _ => None,
    }
  }

  #[test]
  fn no_filters_keeps_unchanged() {
    let e = engine(&[]);
    assert_eq!(e.filter(&text_item("x", None), BUDGET), Verdict::keep());
  }

  #[test]
  fn item_fields_are_visible() {
    let e = engine(&[(
      "a.lua",
      r#"spool.filter(function(item)
           assert(item.text == "hello")
           assert(item.source_app == "org.kde.konsole")
           assert(item.selection == "clipboard")
           assert(item.created_at == 1700000000000)
           assert(item.id == nil)
           assert(#item.mimes == 2 and item.mimes[2] == "UTF8_STRING")
           assert(item:get("UTF8_STRING") == "hello")
           assert(item:get("image/png") == nil)
           return spool.keep{ tags = { "ok" } }
         end)"#,
    )]);
    let v = e.filter(&text_item("hello", Some("org.kde.konsole")), BUDGET);
    assert_eq!(
      v,
      Verdict::Keep { tags: vec!["ok".into()], reps: None, original: true, by: vec![] }
    );
  }

  #[test]
  fn drop_ends_the_chain() {
    let e = engine(&[
      (
        "10.lua",
        r#"spool.filter(function(i) if i.text:find("^drop") then return spool.drop() end end)"#,
      ),
      ("20.lua", r#"spool.filter(function(i) error("must not run") end)"#),
    ]);
    assert_eq!(e.filter(&text_item("drop me", None), BUDGET), Verdict::Drop);
    assert_eq!(e.filter(&text_item("keep me", None), BUDGET), Verdict::keep());
  }

  #[test]
  fn rewrites_and_tags_compose() {
    let e = engine(&[
      (
        "10.lua",
        r#"spool.filter(function(i) return spool.keep{ text = i.text:upper(), tags = {"a"} } end)"#,
      ),
      (
        "20.lua",
        r#"spool.filter(function(i)
          assert(#i.mimes == 1, "aliases are gone after a rewrite")
          assert(i.tags[1] == "a")
          return spool.keep{ text = i.text .. "!", tags = {"b"} }
        end)"#,
      ),
    ]);
    let v = e.filter(&text_item("hi", None), BUDGET);
    assert_eq!(rewritten_text(&v).as_deref(), Some("HI!"));
    let Verdict::Keep { tags, reps, original, by } = v else { panic!() };
    assert_eq!(tags, ["a", "b"]);
    assert_eq!(reps.unwrap()[0].mime, TEXT_MIME);
    assert!(original);
    assert_eq!(by, ["10.lua", "20.lua"]);
  }

  #[test]
  fn one_redacting_filter_drops_the_original() {
    let e = engine(&[
      (
        "10.lua",
        r#"spool.filter(function(i) return spool.keep{ text = i.text:gsub("abc", "[x]"), original = false } end)"#,
      ),
      ("20.lua", r#"spool.filter(function(i) return spool.keep{ text = i.text .. "!" } end)"#),
      (
        "30.lua",
        r#"spool.filter(function(i) return spool.keep{ tags = {"t"}, original = true } end)"#,
      ),
    ]);
    let Verdict::Keep { reps, original, by, .. } = e.filter(&text_item("abc", None), BUDGET) else {
      panic!()
    };
    assert_eq!(reps.unwrap()[0].data, b"[x]!");
    assert!(!original);
    assert_eq!(by, ["10.lua", "20.lua"], "only the scripts that rewrote");
  }

  #[test]
  fn reps_rewrite_keeps_order() {
    let e = engine(&[(
      "a.lua",
      r#"spool.filter(function(i)
           return spool.keep{ reps = { { "text/html", "<b>x</b>" }, { mime = "text/plain", data = "x" } } }
         end)"#,
    )]);
    let Verdict::Keep { reps: Some(reps), .. } = e.filter(&text_item("x", None), BUDGET) else {
      panic!()
    };
    assert_eq!(
      reps.iter().map(|r| r.mime.as_str()).collect::<Vec<_>>(),
      ["text/html", "text/plain"]
    );
  }

  #[test]
  fn errors_and_bad_returns_are_skipped() {
    let e = engine(&[
      ("10.lua", r#"spool.filter(function(i) error("secret " .. i.text) end)"#),
      ("20.lua", r#"spool.filter(function(i) return 42 end)"#),
      ("30.lua", r#"spool.filter(function(i) return spool.keep{ tags = {"ran"} } end)"#),
    ]);
    assert_eq!(
      e.filter(&text_item("x", None), BUDGET),
      Verdict::Keep { tags: vec!["ran".into()], reps: None, original: true, by: vec![] }
    );
  }

  #[test]
  fn endless_filter_times_out_as_unfiltered() {
    let e = engine(&[
      ("10.lua", r#"spool.filter(function(i) return spool.keep{ text = "changed" } end)"#),
      ("20.lua", r#"spool.filter(function(i) while true do end end)"#),
    ]);
    let started = Instant::now();
    assert_eq!(e.filter(&text_item("x", None), Duration::from_millis(100)), Verdict::keep());
    assert!(started.elapsed() < Duration::from_millis(500));
    // The state is still usable afterwards.
    assert_eq!(e.filter_count(), 2);
  }

  #[test]
  fn failed_load_rolls_back_its_filters() {
    let mut e = engine(&[]);
    let why = e
      .load_source("bad.lua", br#"spool.filter(function() end) error("boom: secret")"#)
      .unwrap_err();
    assert_eq!(why, "runtime error at bad.lua:1");
    assert!(!why.contains("secret"));
    assert_eq!(e.filter_count(), 0);
    let why = e.load_source("syntax.lua", b"local = 1").unwrap_err();
    assert!(why.starts_with("syntax error at syntax.lua:1"), "{why}");
    let why = e.load_source("loop.lua", b"while true do end").unwrap_err();
    assert!(why.starts_with("time budget exceeded"), "{why}");
  }

  #[test]
  fn filter_registration_only_while_loading() {
    let e = engine(&[("a.lua", r#"spool.filter(function(i) spool.filter(function() end) end)"#)]);
    assert_eq!(e.filter(&text_item("x", None), BUDGET), Verdict::keep());
    assert_eq!(e.filter_count(), 1);
  }

  #[test]
  fn load_dir_requires_the_store_and_sorts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
      dir.path().join("20-b.lua"),
      r#"spool.filter(function(i) return spool.keep{tags={"b"}} end)"#,
    )
    .unwrap();
    std::fs::write(
      dir.path().join("10-a.lua"),
      r#"spool.filter(function(i) return spool.keep{tags={"a"}} end)"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a script").unwrap();
    std::fs::write(dir.path().join("30-bad.lua"), "this is not lua").unwrap();
    let mut e = Engine::new(false).unwrap();
    assert!(matches!(e.load_dir(dir.path(), false), Err(LoadError::NotInStore(_))));
    let report = e.load_dir(dir.path(), true).unwrap();
    assert_eq!(report.loaded, ["10-a.lua", "20-b.lua"]);
    assert_eq!(report.failed, ["30-bad.lua"]);
    let Verdict::Keep { tags, .. } = e.filter(&text_item("x", None), BUDGET) else { panic!() };
    assert_eq!(tags, ["a", "b"]);
  }
}
