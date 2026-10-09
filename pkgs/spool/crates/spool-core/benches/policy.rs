//! Ingest-policy benchmarks: secret detection, phase-2 `evaluate` and
//! phase-1 `plan`.
//!
//! Run with `cargo bench -p spool-core --bench policy`. Inputs are generated
//! deterministically in-process (no files, no network). Every non-secret
//! input is checked once up front to really be non-secret (and the
//! secret-at-the-end input to really be a secret), otherwise an early match
//! would make the numbers meaningless.

use std::hint::black_box;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use spool_core::config::Config;
use spool_core::item::{Representation, Selection};
use spool_core::policy::{
  Decision, FetchPlan, FetchSpec, OfferInfo, Policy, PolicyState, SecretKind,
};

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
/// 1 KiB .. 8 MiB (the default `max_rep_bytes`).
const SIZES: &[(usize, &str)] =
  &[(KIB, "1KiB"), (64 * KIB, "64KiB"), (MIB, "1MiB"), (8 * MIB, "8MiB")];

// ---- deterministic input generation ----------------------------------------

/// xorshift64*: tiny deterministic PRNG so inputs are identical across runs.
struct Rng(u64);

impl Rng {
  fn next(&mut self) -> u64 {
    self.0 ^= self.0 >> 12;
    self.0 ^= self.0 << 25;
    self.0 ^= self.0 >> 27;
    self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
  }
  fn below(&mut self, n: usize) -> usize {
    (self.next() % n as u64) as usize
  }
  fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
    xs[self.below(xs.len())]
  }
}

const WORDS: &[&str] = &[
  "the",
  "clipboard",
  "history",
  "is",
  "stored",
  "in",
  "a",
  "small",
  "database",
  "and",
  "each",
  "item",
  "keeps",
  "its",
  "original",
  "representations",
  "so",
  "that",
  "pasting",
  "later",
  "works",
  "exactly",
  "like",
  "before",
  "when",
  "source",
  "application",
  "exits",
  "daemon",
  "serves",
  "requests",
  "over",
  "socket",
  "with",
  "bounded",
  "queue",
  "retention",
  "sweep",
  "runs",
  "hourly",
  "pinned",
  "items",
  "are",
  "exempt",
  "from",
  "it",
  "sk-learn",
  "compatible",
  "token",
  "key",
  "secret",
  "value",
  "eyJ",
  "base64",
  "AKIA",
  "ghp",
  "xoxb",
  "BEGIN",
  "PRIVATE",
  "1234",
];

const CODE: &[&str] = &[
  "fn main() -> Result<(), Box<dyn std::error::Error>> {",
  "  let store = Store::open(&path, None)?;",
  "  for (i, rep) in item.reps.iter().enumerate() {",
  "    if rep.data.len() > cap { continue; }",
  "  }",
  "}",
  "const MAX_ITEMS: u32 = 5000;",
  "#[derive(Debug, Clone, PartialEq, Eq)]",
  "pub struct OfferInfo { selection: Selection, mimes: Vec<String> }",
  "    return Err(Error::Policy(format!(\"bad entry {p:?}\")));",
  "import os, sys; print(os.environ.get(\"HOME\", \"/tmp\"))",
  "SELECT id, hash FROM items WHERE selection = ?1 ORDER BY last_used_at DESC LIMIT 1;",
  "curl -fsSL https://example.org/api/v1/items?limit=50&offset=100 | jq '.items[]'",
  "{\"id\": 42, \"name\": \"spool\", \"tags\": [\"a\", \"b\"], \"size\": 1048576}",
  "// TODO(bcnelson): handle the 2024-03-11T12:34:56Z timestamp edge case",
];

const UNICODE_WORDS: &[&str] = &[
  "café",
  "naïve",
  "Größe",
  "über",
  "façade",
  "smörgåsbord",
  "日本語",
  "テキスト",
  "中文",
  "Ελληνικά",
  "русский",
  "текст",
  "emoji",
  "🦀",
  "✓",
  "→",
  "clipboard",
  "history",
  "€42",
  "ß",
];

/// Realistic non-secret text: paragraphs of prose interleaved with code.
fn prose_code(len: usize, seed: u64) -> String {
  let mut r = Rng(seed);
  let mut s = String::with_capacity(len + 256);
  while s.len() < len {
    if r.below(3) == 0 {
      for _ in 0..1 + r.below(6) {
        s.push_str(r.pick(CODE));
        s.push('\n');
      }
    } else {
      for i in 0..20 + r.below(60) {
        if i > 0 {
          s.push(' ');
        }
        s.push_str(r.pick(WORDS));
      }
      s.push_str(".\n\n");
    }
  }
  truncate_at_char(s, len)
}

/// Non-ASCII prose (exercises Unicode `\b` handling in the regex engine).
fn unicode_prose(len: usize, seed: u64) -> String {
  let mut r = Rng(seed);
  let mut s = String::with_capacity(len + 64);
  while s.len() < len {
    s.push_str(r.pick(UNICODE_WORDS));
    s.push(if r.below(12) == 0 { '\n' } else { ' ' });
  }
  truncate_at_char(s, len)
}

fn truncate_at_char(mut s: String, len: usize) -> String {
  let mut cut = len.min(s.len());
  while !s.is_char_boundary(cut) {
    cut -= 1;
  }
  s.truncate(cut);
  s
}

/// `unit` repeated until `len` bytes.
fn repeat_to(unit: &str, len: usize) -> String {
  truncate_at_char(unit.repeat(len / unit.len() + 1), len)
}

/// Random base64 alphabet in 76-char lines (looks like key material, but
/// none of the token prefixes are formed with their required context).
fn base64ish(len: usize, seed: u64) -> String {
  const ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789+/";
  let mut r = Rng(seed);
  let mut s = String::with_capacity(len);
  while s.len() < len {
    for _ in 0..76 {
      s.push(ALPHA[r.below(ALPHA.len())] as char);
    }
    s.push('\n');
  }
  truncate_at_char(s, len)
}

/// Named non-secret inputs. Each must be classified as `None`.
fn inputs(len: usize) -> Vec<(&'static str, String)> {
  vec![
    ("prose_code", prose_code(len, 0x5eed)),
    ("unicode_prose", unicode_prose(len, 0xface)),
    // `sk-` followed by 20+ key chars but no digit: every occurrence matches
    // RE_SK and has to go through `sk_body_is_key`.
    ("sk_runs", repeat_to(" sk-abcdefghijklmnopqrstuvwxyz", len)),
    // One huge `sk-` body (no digit): a single very long capture.
    ("sk_long_body", format!(" sk-{}", "a".repeat(len.saturating_sub(4)))),
    // JWT-shaped prefixes missing the third segment.
    ("jwt_partial", repeat_to(" eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0 ", len)),
    ("eyJ_run", repeat_to("eyJ", len)),
    ("digits", repeat_to("0123456789", len)),
    ("base64ish", base64ish(len, 0xb64)),
    // PEM header that never becomes a private key.
    ("pem_partial", repeat_to("-----BEGIN ABCDEFGHIJ KLMNOPQRS TUVWXYZ0123 ", len)),
    // Prefixes of every token pattern, each one character short.
    (
      "token_prefixes",
      repeat_to(
        " ghp_abcdefghijklmnopqrstuvwxyz012345678 AKIAABCDEFGHIJKLMNO xoxb-1234567890-abc \
         AGE-SECRET-KEY-1ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        len,
      ),
    ),
  ]
}

/// Realistic text with a Slack token at the very end (the last pattern
/// checked before OTP, so every earlier regex scans the whole text first).
fn secret_at_end(len: usize) -> String {
  const TOKEN: &str = "\nxoxb-123456789012-abcdefghijKLMNOP\n";
  let mut s = prose_code(len - TOKEN.len(), 0x5eed);
  s.push_str(TOKEN);
  s
}

fn policy() -> Policy {
  Policy::new(Config::default(), [7; 32]).unwrap()
}

fn offer(mimes: &[&str]) -> OfferInfo {
  OfferInfo {
    selection: Selection::Clipboard,
    mimes: mimes.iter().map(|m| m.to_string()).collect(),
    source_app: None,
    at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_000_000),
  }
}

const FIREFOX: &[&str] = &[
  "text/html",
  "text/_moz_htmlcontext",
  "text/_moz_htmlinfo",
  "text/x-moz-url-priv",
  "text/plain;charset=utf-8",
  "text/plain",
  "UTF8_STRING",
  "COMPOUND_TEXT",
  "TEXT",
  "STRING",
];

const KDE_QT: &[&str] = &[
  "text/plain",
  "text/plain;charset=utf-8",
  "UTF8_STRING",
  "TEXT",
  "STRING",
  "COMPOUND_TEXT",
  "text/html",
  "application/x-qt-richtext",
  "application/vnd.oasis.opendocument.text",
  "image/png",
  "application/x-qt-image",
  "SAVE_TARGETS",
];

/// 256 offered mimes: case-variant text duplicates, many allowlisted
/// `image/*` types, long junk parameters and unlisted types.
fn hostile_mimes() -> Vec<String> {
  let mut v = Vec::with_capacity(256);
  for i in 0..256 {
    v.push(match i % 8 {
      0 => "TEXT/PLAIN;CHARSET=UTF-8".to_string(),
      1 => format!("image/x-hostile-{i}"),
      2 => format!("application/x-junk-{i};{}", "p=".repeat(64)),
      3 => "Text/Plain".to_string(),
      4 => format!("image/x-hostile-{i};param={i}"),
      5 => format!("x-special/thing-{i}"),
      6 => "image/png".to_string(),
      _ => format!("text/x-{i}"),
    });
  }
  v
}

fn fetch_spec(p: &Policy, o: &OfferInfo) -> FetchSpec {
  match p.plan(&mut PolicyState::new(), o) {
    FetchPlan::Fetch(spec) => spec,
    other => panic!("expected Fetch, got {other:?}"),
  }
}

// ---- benches ---------------------------------------------------------------

fn bench_detect_secret(c: &mut Criterion) {
  let p = policy();
  let mut g = c.benchmark_group("detect_secret");
  for &(len, label) in SIZES {
    if len >= MIB {
      g.sample_size(10).measurement_time(Duration::from_secs(4));
    } else {
      g.sample_size(30).measurement_time(Duration::from_secs(2));
    }
    g.throughput(Throughput::Bytes(len as u64));
    for (name, text) in inputs(len) {
      assert_eq!(p.detect_secret(&text), None, "{name}/{label} must not be a secret");
      g.bench_with_input(BenchmarkId::new(name, label), &text, |b, t| {
        b.iter(|| p.detect_secret(black_box(t)))
      });
    }
    let text = secret_at_end(len);
    assert_eq!(p.detect_secret(&text), Some(SecretKind::SlackToken));
    g.bench_with_input(BenchmarkId::new("secret_at_end", label), &text, |b, t| {
      b.iter(|| p.detect_secret(black_box(t)))
    });
  }
  g.finish();
}

fn bench_evaluate(c: &mut Criterion) {
  let p = policy();
  let o = offer(FIREFOX);
  let text_spec = fetch_spec(&p, &offer(&["text/plain;charset=utf-8", "text/plain", "TEXT"]));
  let png_spec = fetch_spec(&p, &offer(&["image/png"]));
  let mut g = c.benchmark_group("evaluate");
  for &(len, label) in SIZES {
    if len >= MIB {
      g.sample_size(10).measurement_time(Duration::from_secs(4));
    } else {
      g.sample_size(30).measurement_time(Duration::from_secs(2));
    }
    g.throughput(Throughput::Bytes(len as u64));
    let cases: Vec<(&str, &FetchSpec, Bytes, bool)> = vec![
      ("text_prose_code", &text_spec, Bytes::from(prose_code(len, 0x5eed)), true),
      ("text_unicode", &text_spec, Bytes::from(unicode_prose(len, 0xface)), true),
      ("text_secret_at_end", &text_spec, Bytes::from(secret_at_end(len)), false),
      // Not textual: no secret scan, only caps / blank check / hash /
      // preview. Baseline for the cost of the scan itself.
      ("png", &png_spec, Bytes::from(base64ish(len, 1).into_bytes()), true),
    ];
    for (name, spec, data, stored) in cases {
      let reps = || vec![Representation::new(spec.mimes[0].clone(), data.clone())];
      let d = p.evaluate(&mut PolicyState::new(), &o, spec, reps());
      assert_eq!(matches!(d, Decision::Store(_)), stored, "{name}/{label}: {d:?}");
      g.bench_function(BenchmarkId::new(name, label), |b| {
        b.iter(|| p.evaluate(&mut PolicyState::new(), &o, spec, black_box(reps())))
      });
    }
  }
  g.finish();
}

fn bench_plan(c: &mut Criterion) {
  let p = policy();
  let wildcard = Policy::new(
    Config {
      mime_allowlist: vec![
        "text/plain;charset=utf-8".into(),
        "text/plain".into(),
        "image/*".into(),
      ],
      ..Config::default()
    },
    [7; 32],
  )
  .unwrap();
  let hostile = OfferInfo { mimes: hostile_mimes(), ..offer(&[]) };
  let mut g = c.benchmark_group("plan");
  g.sample_size(50).measurement_time(Duration::from_secs(2));
  let cases: [(&str, &Policy, OfferInfo); 4] = [
    ("firefox_10", &p, offer(FIREFOX)),
    ("kde_qt_12", &p, offer(KDE_QT)),
    ("hostile_256", &p, hostile.clone()),
    ("hostile_256_image_wildcard", &wildcard, hostile),
  ];
  for (name, pol, o) in &cases {
    g.throughput(Throughput::Elements(o.mimes.len() as u64));
    assert!(matches!(pol.plan(&mut PolicyState::new(), o), FetchPlan::Fetch(_)));
    g.bench_function(*name, |b| b.iter(|| pol.plan(&mut PolicyState::new(), black_box(o))));
  }
  g.finish();
}

criterion_group!(benches, bench_detect_secret, bench_evaluate, bench_plan);
criterion_main!(benches);
