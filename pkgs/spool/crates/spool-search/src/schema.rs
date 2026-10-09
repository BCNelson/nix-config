//! Index schema, the `clip_text` analyzer and the `clip_tri` trigram
//! analyzer.

use tantivy::Index;
use tantivy::schema::{
  DateOptions, DateTimePrecision, FAST, FacetOptions, Field, INDEXED, IndexRecordOption, STORED,
  STRING, Schema, TextFieldIndexing, TextOptions,
};
use tantivy::tokenizer::{
  AsciiFoldingFilter, BoxTokenStream, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
  Token, TokenStream, Tokenizer,
};

/// Bumped whenever the schema or the analyzer changes; a mismatch with the
/// commit payload forces a rebuild.
///
/// - 1: `text` only (M3).
/// - 2: adds `text_tri`, per-word trigrams for substring matching (M7).
pub const SCHEMA_VERSION: u32 = 2;

/// Name of the text analyzer: `SimpleTokenizer -> RemoveLongFilter(64) ->
/// LowerCaser -> AsciiFoldingFilter`.
pub const TOKENIZER: &str = "clip_text";

/// Name of the trigram analyzer over `text_tri`: the `clip_text` words,
/// each split into its overlapping 3-character windows.
pub const TRIGRAM_TOKENIZER: &str = "clip_tri";

/// Tokens longer than this many bytes are dropped (hashes, base64 blobs).
pub(crate) const MAX_TOKEN_LEN: usize = 64;

/// Trigram width, in characters.
pub(crate) const GRAM: usize = 3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Fields {
  pub id: Field,
  pub text: Field,
  pub text_tri: Field,
  pub mime: Field,
  pub app: Field,
  pub tag: Field,
  pub created: Field,
  pub pinned: Field,
}

pub(crate) fn analyzer() -> TextAnalyzer {
  TextAnalyzer::builder(SimpleTokenizer::default())
    .filter(RemoveLongFilter::limit(MAX_TOKEN_LEN))
    .filter(LowerCaser)
    .filter(AsciiFoldingFilter)
    .build()
}

/// The 3-character windows of `word` (by `char`), in order. Empty for words
/// shorter than [`GRAM`].
pub(crate) fn trigrams(word: &str) -> Vec<&str> {
  let bounds: Vec<usize> = word.char_indices().map(|(i, _)| i).chain([word.len()]).collect();
  bounds.windows(GRAM + 1).map(|w| &word[w[0]..w[GRAM]]).collect()
}

/// Per-word trigram tokenizer: runs `clip_text`, then emits every trigram of
/// each word. Trigrams of one word sit at consecutive positions; a gap of one
/// position separates words, so a phrase over trigrams never spans a word
/// boundary. Words shorter than [`GRAM`] produce nothing. Streams lazily
/// (no per-document buffer of tokens).
#[derive(Clone)]
pub(crate) struct TrigramTokenizer {
  words: TextAnalyzer,
}

impl TrigramTokenizer {
  pub(crate) fn new() -> Self {
    TrigramTokenizer { words: analyzer() }
  }
}

pub(crate) struct TrigramStream<'a> {
  words: BoxTokenStream<'a>,
  /// Current word (folded) and its char boundaries.
  word: String,
  bounds: Vec<usize>,
  /// Next window to emit in `word`.
  next: usize,
  /// Position of the current word's first trigram.
  base: usize,
  /// Position for the next word's first trigram.
  next_base: usize,
  token: Token,
}

impl Tokenizer for TrigramTokenizer {
  type TokenStream<'a> = TrigramStream<'a>;

  fn token_stream<'a>(&'a mut self, text: &'a str) -> TrigramStream<'a> {
    TrigramStream {
      words: self.words.token_stream(text),
      word: String::new(),
      bounds: vec![],
      next: 0,
      base: 0,
      next_base: 0,
      token: Token::default(),
    }
  }
}

impl TokenStream for TrigramStream<'_> {
  fn advance(&mut self) -> bool {
    loop {
      if self.next + GRAM < self.bounds.len() {
        let (from, to) = (self.bounds[self.next], self.bounds[self.next + GRAM]);
        self.token.text.clear();
        self.token.text.push_str(&self.word[from..to]);
        self.token.position = self.base + self.next;
        self.token.position_length = 1;
        self.next += 1;
        return true;
      }
      if !self.words.advance() {
        return false;
      }
      let w = self.words.token();
      self.token.offset_from = w.offset_from;
      self.token.offset_to = w.offset_to;
      self.word.clear();
      self.word.push_str(&w.text);
      self.bounds.clear();
      self.bounds.extend(self.word.char_indices().map(|(i, _)| i).chain([self.word.len()]));
      self.next = 0;
      let grams = self.bounds.len().saturating_sub(GRAM);
      if grams > 0 {
        self.base = self.next_base;
        // +1: leave a hole so adjacent words' trigrams are never adjacent.
        self.next_base = self.base + grams + 1;
      }
    }
  }

  fn token(&self) -> &Token {
    &self.token
  }

  fn token_mut(&mut self) -> &mut Token {
    &mut self.token
  }
}

pub(crate) fn build() -> (Schema, Fields) {
  let mut b = Schema::builder();
  let id = b.add_u64_field("id", INDEXED | FAST | STORED);
  let text_opts = TextOptions::default().set_indexing_options(
    TextFieldIndexing::default()
      .set_tokenizer(TOKENIZER)
      .set_index_option(IndexRecordOption::WithFreqsAndPositions),
  );
  // Not stored: the text lives (encrypted) in the store only.
  let text = b.add_text_field("text", text_opts);
  // Substring matching: positions for phrase queries over a word's
  // trigrams; scored as a constant, so no fieldnorms. Not stored.
  let tri_opts = TextOptions::default().set_indexing_options(
    TextFieldIndexing::default()
      .set_tokenizer(TRIGRAM_TOKENIZER)
      .set_fieldnorms(false)
      .set_index_option(IndexRecordOption::WithFreqsAndPositions),
  );
  let text_tri = b.add_text_field("text_tri", tri_opts);
  let mime = b.add_text_field("mime", STRING);
  let app = b.add_text_field("app", STRING);
  let tag = b.add_facet_field("tag", FacetOptions::default());
  let created = b.add_date_field(
    "created",
    DateOptions::default().set_fast().set_precision(DateTimePrecision::Milliseconds),
  );
  let pinned = b.add_bool_field("pinned", INDEXED | FAST);
  (b.build(), Fields { id, text, text_tri, mime, app, tag, created, pinned })
}

/// Tokenizers are not persisted; register ours on every open/create.
pub(crate) fn register(index: &Index) {
  index.tokenizers().register(TOKENIZER, analyzer());
  index.tokenizers().register(TRIGRAM_TOKENIZER, TextAnalyzer::from(TrigramTokenizer::new()));
}

#[cfg(test)]
mod tests {
  use super::*;
  use tantivy::tokenizer::TokenStream;

  fn tokens(s: &str) -> Vec<String> {
    let mut a = analyzer();
    let mut ts = a.token_stream(s);
    let mut out = vec![];
    while ts.advance() {
      out.push(ts.token().text.clone());
    }
    out
  }

  fn grams(s: &str) -> Vec<(String, usize)> {
    let mut a = TrigramTokenizer::new();
    let mut ts = a.token_stream(s);
    let mut out = vec![];
    while ts.advance() {
      out.push((ts.token().text.clone(), ts.token().position));
    }
    out
  }

  #[test]
  fn trigram_pipeline() {
    let g = grams("Kubectl ab GET café");
    let want = [
      ("kub", 0),
      ("ube", 1),
      ("bec", 2),
      ("ect", 3),
      ("ctl", 4),
      // "ab" is too short; "get" starts after a one-position gap.
      ("get", 6),
      ("caf", 8),
      ("afe", 9),
    ];
    assert_eq!(g, want.map(|(t, p)| (t.to_string(), p)));
    assert_eq!(grams("日本語の"), [("日本語".to_string(), 0), ("本語の".to_string(), 1)]);
    assert!(grams("a bc").is_empty());
    assert!(grams(&"x".repeat(80)).is_empty());
    assert_eq!(trigrams("abcd"), ["abc", "bcd"]);
    assert_eq!(trigrams("éèàù"), ["éèà", "èàù"]);
    assert!(trigrams("ab").is_empty());
  }

  #[test]
  fn analyzer_pipeline() {
    assert_eq!(tokens("Kubectl GET-pods Café"), ["kubectl", "get", "pods", "cafe"]);
    let long = "a".repeat(80);
    assert_eq!(tokens(&format!("x {long} y")), ["x", "y"]);
  }
}
