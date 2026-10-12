//! The `spool` global: hooks, verdicts, logging and text helpers.
//!
//! ```lua
//! spool.filter(function(item) ... end)  -- load time only
//! spool.keep{ tags = {..}, text = ".." } / spool.keep{ reps = { {mime, data}, .. } } / spool.drop()
//! spool.log(...)                         -- also what `print` does
//! spool.regex(pattern)                   -- :is_match :match :captures :match_all :replace :split
//! spool.json.decode(s) / spool.json.encode(v [, { pretty = true }])
//! spool.url.parse(s) / spool.url.build(t)
//! ```

use mlua::{
  Function, Lua, LuaSerdeExt, MultiValue, Table, UserData, UserDataMethods, Value, Variadic,
};
use spool_proto::{Verdict, WireRep};

use crate::engine::{LuaVerdict, Registry, TEXT_MIME};

/// Compiled-regex size cap (a script's pattern, not user input, but still).
const REGEX_SIZE_LIMIT: usize = 1 << 20;

pub fn install(lua: &Lua) -> mlua::Result<()> {
  let spool = lua.create_table()?;

  spool.set(
    "filter",
    lua.create_function(|lua, f: Function| {
      let mut reg = lua.app_data_mut::<Registry>().expect("registry");
      if !reg.loading {
        return Err(mlua::Error::runtime("spool.filter can only be called while a script loads"));
      }
      let script = reg.current.clone().unwrap_or_default();
      reg.filters.push(crate::engine::Filter { script, func: f });
      Ok(())
    })?,
  )?;

  spool.set("keep", lua.create_function(|_, opts: Option<Table>| keep(opts).map(LuaVerdict))?)?;
  spool.set("drop", lua.create_function(|_, ()| Ok(LuaVerdict(Verdict::Drop)))?)?;

  let log = lua.create_function(|lua, args: Variadic<Value>| {
    let msg = args.iter().map(|v| tostring(lua, v)).collect::<mlua::Result<Vec<_>>>()?.join("\t");
    let script = lua.app_data_ref::<Registry>().and_then(|r| r.current.clone()).unwrap_or_default();
    tracing::info!(script = %script, "{msg}");
    Ok(())
  })?;
  spool.set("log", log.clone())?;
  // stdout is the journal too; route print through the same place.
  lua.globals().set("print", log)?;

  spool.set("regex", lua.create_function(|_, pat: String| regex(&pat))?)?;
  spool.set("json", json(lua)?)?;
  spool.set("url", url(lua)?)?;

  lua.globals().set("spool", spool)
}

fn tostring(lua: &Lua, v: &Value) -> mlua::Result<String> {
  let f: Function = lua.globals().get("tostring")?;
  let s: mlua::LuaString = f.call(v.clone())?;
  Ok(s.to_string_lossy())
}

/// `spool.keep{ tags, text | reps }`.
fn keep(opts: Option<Table>) -> mlua::Result<Verdict> {
  let Some(opts) = opts else { return Ok(Verdict::keep()) };
  let tags: Vec<String> = opts.get::<Option<Vec<String>>>("tags")?.unwrap_or_default();
  let text: Option<mlua::LuaString> = opts.get("text")?;
  let reps: Option<Table> = opts.get("reps")?;
  let reps = match (text, reps) {
    (Some(_), Some(_)) => {
      return Err(mlua::Error::runtime("spool.keep: give text or reps, not both"));
    }
    (Some(t), None) => Some(vec![WireRep { mime: TEXT_MIME.into(), data: t.as_bytes().to_vec() }]),
    (None, Some(list)) => {
      let mut out = Vec::new();
      for entry in list.sequence_values::<Table>() {
        let entry = entry?;
        let mime: Option<String> = entry.get("mime")?;
        let data: Option<mlua::LuaString> = entry.get("data")?;
        let (mime, data) = match (mime, data) {
          (Some(m), Some(d)) => (m, d),
          _ => (entry.get::<String>(1)?, entry.get::<mlua::LuaString>(2)?),
        };
        out.push(WireRep { mime, data: data.as_bytes().to_vec() });
      }
      if out.is_empty() {
        return Err(mlua::Error::runtime("spool.keep: reps is empty"));
      }
      Some(out)
    }
    (None, None) => None,
  };
  // `original = false`: a redacting rewrite; the copy as it came in is
  // not kept as an earlier version.
  // Strictly a boolean: `original = "no"` must not mean "keep it".
  let original = match opts.get::<Value>("original")? {
    Value::Nil => true,
    Value::Boolean(b) => b,
    _ => return Err(mlua::Error::runtime("spool.keep: original must be true or false")),
  };
  Ok(Verdict::Keep { tags, reps, original, by: Vec::new() })
}

/// A compiled `spool.regex` (byte-oriented: Lua strings are bytes).
struct LuaRegex(regex::bytes::Regex);

fn regex(pat: &str) -> mlua::Result<LuaRegex> {
  regex::bytes::RegexBuilder::new(pat)
    .size_limit(REGEX_SIZE_LIMIT)
    .build()
    .map(LuaRegex)
    .map_err(|e| mlua::Error::runtime(format!("spool.regex: {e}")))
}

impl UserData for LuaRegex {
  fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
    m.add_method("is_match", |_, re, s: mlua::LuaString| Ok(re.0.is_match(&s.as_bytes())));
    m.add_method("match", |lua, re, s: mlua::LuaString| {
      re.0.find(&s.as_bytes()).map(|x| lua.create_string(x.as_bytes())).transpose()
    });
    m.add_method("match_all", |lua, re, s: mlua::LuaString| {
      let b = s.as_bytes();
      let found = re.0.find_iter(&b).map(|x| lua.create_string(x.as_bytes()));
      lua.create_sequence_from(found.collect::<mlua::Result<Vec<_>>>()?)
    });
    m.add_method("captures", |lua, re, s: mlua::LuaString| {
      let b = s.as_bytes();
      let Some(caps) = re.0.captures(&b) else { return Ok(Value::Nil) };
      Ok(Value::Table(captures_table(lua, &re.0, &caps)?))
    });
    m.add_method("split", |lua, re, s: mlua::LuaString| {
      let b = s.as_bytes();
      let parts = re.0.split(&b).map(|p| lua.create_string(p));
      lua.create_sequence_from(parts.collect::<mlua::Result<Vec<_>>>()?)
    });
    // rep: a string with $1 / ${name} references, or a function getting the
    // captures table and returning the replacement.
    m.add_method("replace", |lua, re, (s, rep): (mlua::LuaString, Value)| {
      let b = s.as_bytes();
      let mut out = Vec::with_capacity(b.len());
      let mut last = 0;
      for caps in re.0.captures_iter(&b) {
        let m = caps.get(0).expect("group 0");
        out.extend_from_slice(&b[last..m.start()]);
        match &rep {
          Value::String(r) => caps.expand(&r.as_bytes(), &mut out),
          Value::Function(f) => {
            let r: mlua::LuaString = f.call(captures_table(lua, &re.0, &caps)?)?;
            out.extend_from_slice(&r.as_bytes());
          }
          _ => return Err(mlua::Error::runtime("replace: rep must be a string or a function")),
        }
        last = m.end();
      }
      out.extend_from_slice(&b[last..]);
      lua.create_string(out)
    });
  }
}

/// `{ [0] = whole, [1] = group 1, ..., name = named group }` (missing groups
/// are absent).
fn captures_table(
  lua: &Lua,
  re: &regex::bytes::Regex,
  caps: &regex::bytes::Captures<'_>,
) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  for (i, name) in re.capture_names().enumerate() {
    if let Some(m) = caps.get(i) {
      let s = lua.create_string(m.as_bytes())?;
      t.raw_set(i, s.clone())?;
      if let Some(name) = name {
        t.raw_set(name, s)?;
      }
    }
  }
  Ok(t)
}

fn json(lua: &Lua) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  t.set(
    "decode",
    lua.create_function(|lua, s: mlua::LuaString| {
      let v: serde_json::Value = serde_json::from_slice(&s.as_bytes())
        .map_err(|e| mlua::Error::runtime(format!("spool.json.decode: {e}")))?;
      lua.to_value(&v)
    })?,
  )?;
  t.set(
    "encode",
    lua.create_function(|_, (v, opts): (Value, Option<Table>)| {
      let pretty =
        opts.map(|o| o.get::<Option<bool>>("pretty")).transpose()?.flatten().unwrap_or(false);
      let r = if pretty { serde_json::to_string_pretty(&v) } else { serde_json::to_string(&v) };
      r.map_err(|e| mlua::Error::runtime(format!("spool.json.encode: {e}")))
    })?,
  )?;
  t.set("null", lua.null())?;
  Ok(t)
}

fn url(lua: &Lua) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  // parse(s) -> table, or nil + error message.
  t.set(
    "parse",
    lua.create_function(|lua, s: String| match url::Url::parse(s.trim()) {
      Ok(u) => Ok(MultiValue::from_vec(vec![Value::Table(url_table(lua, &u)?)])),
      Err(e) => {
        Ok(MultiValue::from_vec(vec![Value::Nil, Value::String(lua.create_string(e.to_string())?)]))
      }
    })?,
  )?;
  t.set("build", lua.create_function(|_, t: Table| url_build(&t))?)?;
  Ok(t)
}

fn url_table(lua: &Lua, u: &url::Url) -> mlua::Result<Table> {
  let t = lua.create_table()?;
  t.set("href", u.as_str())?;
  t.set("scheme", u.scheme())?;
  t.set("username", u.username())?;
  t.set("password", u.password())?;
  t.set("host", u.host_str())?;
  t.set("port", u.port())?;
  t.set("path", u.path())?;
  t.set("query_string", u.query())?;
  t.set("query", query_pairs(lua, u)?)?;
  t.set("fragment", u.fragment())?;
  Ok(t)
}

/// `{ {key, value}, ... }` in URL order.
fn query_pairs(lua: &Lua, u: &url::Url) -> mlua::Result<Table> {
  let pairs = u
    .query_pairs()
    .map(|(k, v)| lua.create_sequence_from([k.into_owned(), v.into_owned()]))
    .collect::<mlua::Result<Vec<_>>>()?;
  lua.create_sequence_from(pairs)
}

/// Apply a (modified) `parse` table to its `href`. The query is re-encoded
/// only if `query` / `query_string` changed, so untouched URLs come back
/// byte for byte.
fn url_build(t: &Table) -> mlua::Result<String> {
  let err = |what: &str| mlua::Error::runtime(format!("spool.url.build: {what}"));
  let href: String =
    t.get::<Option<String>>("href")?.ok_or_else(|| err("needs a table from spool.url.parse"))?;
  let orig = url::Url::parse(&href).map_err(|e| err(&e.to_string()))?;
  let mut u = orig.clone();
  let scheme: String = t.get("scheme")?;
  if scheme != u.scheme() {
    u.set_scheme(&scheme).map_err(|()| err("cannot change the scheme to that"))?;
  }
  let host: Option<String> = t.get("host")?;
  if host.as_deref() != u.host_str() {
    u.set_host(host.as_deref()).map_err(|e| err(&e.to_string()))?;
  }
  let port: Option<u16> = t.get("port")?;
  if port != u.port() {
    u.set_port(port).map_err(|()| err("this URL cannot have a port"))?;
  }
  let username: String = t.get::<Option<String>>("username")?.unwrap_or_default();
  if username != u.username() {
    u.set_username(&username).map_err(|()| err("this URL cannot have a username"))?;
  }
  let password: Option<String> = t.get("password")?;
  if password.as_deref() != u.password() {
    u.set_password(password.as_deref()).map_err(|()| err("this URL cannot have a password"))?;
  }
  let path: String = t.get("path")?;
  if path != u.path() {
    u.set_path(&path);
  }
  let qs: Option<String> = t.get("query_string")?;
  let pairs: Vec<(String, String)> = match t.get::<Option<Table>>("query")? {
    Some(list) => list
      .sequence_values::<Table>()
      .map(|p| {
        let p = p?;
        Ok((p.get::<String>(1)?, p.get::<String>(2)?))
      })
      .collect::<mlua::Result<_>>()?,
    None => Vec::new(),
  };
  let orig_pairs: Vec<(String, String)> =
    orig.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
  if qs.as_deref() != orig.query() {
    u.set_query(qs.as_deref().filter(|q| !q.is_empty()));
  } else if pairs != orig_pairs {
    if pairs.is_empty() {
      u.set_query(None);
    } else {
      u.query_pairs_mut().clear().extend_pairs(pairs);
    }
  }
  let fragment: Option<String> = t.get("fragment")?;
  if fragment.as_deref() != u.fragment() {
    u.set_fragment(fragment.as_deref());
  }
  Ok(u.into())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn lua() -> Lua {
    let lua = Lua::new();
    lua.set_app_data(Registry::default());
    install(&lua).unwrap();
    lua
  }

  fn eval<T: mlua::FromLuaMulti>(lua: &Lua, src: &str) -> T {
    lua.load(src).eval().unwrap_or_else(|e| panic!("{src}: {e}"))
  }

  #[test]
  fn regex_methods() {
    let lua = lua();
    assert!(eval::<bool>(&lua, r#"return spool.regex("^utm_"):is_match("utm_source")"#));
    assert_eq!(eval::<String>(&lua, r#"return spool.regex("\\d+"):match("ab 123 45")"#), "123");
    assert_eq!(eval::<Option<String>>(&lua, r#"return spool.regex("\\d+"):match("none")"#), None);
    assert_eq!(
      eval::<String>(
        &lua,
        r#"return table.concat(spool.regex("\\d+"):match_all("1 22 333"), ",")"#
      ),
      "1,22,333"
    );
    assert_eq!(
      eval::<String>(
        &lua,
        r#"local c = spool.regex("(?P<k>\\w+)=(\\w+)"):captures("a=b"); return c[0]..c.k..c[2]"#
      ),
      "a=bab"
    );
    assert_eq!(
      eval::<String>(&lua, r#"return spool.regex("(\\w+)@"):replace("me@x you@y", "<$1>")"#),
      "<me>x <you>y"
    );
    assert_eq!(
      eval::<String>(
        &lua,
        r#"return spool.regex("\\d"):replace("a1b2", function(c) return "[" .. c[0] * 2 .. "]" end)"#
      ),
      "a[2]b[4]"
    );
    assert_eq!(
      eval::<String>(&lua, r#"return table.concat(spool.regex(",\\s*"):split("a, b,c"), "|")"#),
      "a|b|c"
    );
    assert!(lua.load(r#"spool.regex("(")"#).exec().is_err());
  }

  #[test]
  fn json_round_trip() {
    let lua = lua();
    assert_eq!(eval::<i64>(&lua, r#"return spool.json.decode('{"a":[1,2,{"b":3}]}').a[3].b"#), 3);
    assert_eq!(
      eval::<String>(&lua, r#"return spool.json.encode(spool.json.decode('[1,"x",true]'))"#),
      r#"[1,"x",true]"#
    );
    assert!(
      eval::<String>(&lua, r#"return spool.json.encode({a=1}, {pretty=true})"#).contains("\n")
    );
    assert!(lua.load(r#"spool.json.decode("{nope")"#).exec().is_err());
  }

  #[test]
  fn url_strip_tracking_params() {
    let lua = lua();
    let out: String = eval(
      &lua,
      r#"
      local u = spool.url.parse("https://example.com/a?id=7&utm_source=x&fbclid=y#top")
      local kept = {}
      for _, kv in ipairs(u.query) do
        if not spool.regex("^(utm_.*|fbclid)$"):is_match(kv[1]) then kept[#kept + 1] = kv end
      end
      u.query = kept
      return spool.url.build(u)
    "#,
    );
    assert_eq!(out, "https://example.com/a?id=7#top");
  }

  #[test]
  fn url_untouched_is_byte_identical() {
    let lua = lua();
    let src = "https://example.com/p?q=a%20b&x=1+2";
    let out: String = eval(&lua, &format!(r#"return spool.url.build(spool.url.parse("{src}"))"#));
    assert_eq!(out, src);
    let (none, msg): (Option<Table>, String) = eval(&lua, r#"return spool.url.parse("not a url")"#);
    assert!(none.is_none() && !msg.is_empty());
    let host: String = eval(
      &lua,
      r#"local u = spool.url.parse("http://a.com/x"); u.host = "b.org"; return spool.url.build(u)"#,
    );
    assert_eq!(host, "http://b.org/x");
  }

  #[test]
  fn keep_validates() {
    let lua = lua();
    assert!(lua.load(r#"spool.keep{ text = "a", reps = {{"text/plain", "a"}} }"#).exec().is_err());
    assert!(lua.load(r#"spool.keep{ reps = {} }"#).exec().is_err());
    assert!(lua.load(r#"spool.keep()"#).exec().is_ok());
    assert!(lua.load(r#"spool.keep{ original = "no" }"#).exec().is_err());
  }
}
