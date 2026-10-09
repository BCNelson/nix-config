//! Round-trip, random-access and tamper tests for the chunked file format.

use std::io::{Cursor, Write};

use proptest::prelude::*;
use spool_crypto::{
  ChunkedReader, ChunkedWriter, DataKey, Error, HEADER_LEN, Label, SubKey, Zeroizing, open_file,
  plaintext_len, seal_file,
};

const TAG: usize = 16;
const LOG2: u8 = 12;
const CS: usize = 1 << LOG2;
const PATH: &[u8] = b"blobs/0001";

fn sub() -> SubKey {
  DataKey::from_bytes(Zeroizing::new([0x42; 32])).derive(Label::Blob)
}

fn data(len: usize) -> Vec<u8> {
  // Cheap deterministic non-repeating-ish bytes.
  let mut x: u32 = 0x1234_5678;
  (0..len)
    .map(|_| {
      x ^= x << 13;
      x ^= x >> 17;
      x ^= x << 5;
      x as u8
    })
    .collect()
}

fn seal_with(sub: &SubKey, path: &[u8], log2: u8, pt: &[u8], writes: usize) -> Vec<u8> {
  let mut w = ChunkedWriter::new(sub, path, log2, Vec::new()).unwrap();
  // Exercise odd write splits.
  let step = (pt.len() / writes.max(1)).max(1);
  for part in pt.chunks(step) {
    w.write_all(part).unwrap();
  }
  w.finish().unwrap()
}

fn seal(pt: &[u8]) -> Vec<u8> {
  seal_with(&sub(), PATH, LOG2, pt, 3)
}

/// Open + read everything; the combined result is what tamper tests check.
fn read(sub: &SubKey, path: &[u8], file: Vec<u8>) -> Result<Vec<u8>, Error> {
  let mut r = ChunkedReader::open(sub, path, Cursor::new(file))?;
  Ok(r.read_all()?.to_vec())
}

fn chunk_off(i: usize) -> usize {
  HEADER_LEN + i * (CS + TAG)
}

const SIZES: [usize; 8] = [0, 1, CS - 1, CS, CS + 1, 2 * CS, 5 * CS + 7, 3 * 1024 * 1024 + 123];

#[test]
fn round_trips() {
  let s = sub();
  for len in SIZES {
    let pt = data(len);
    let file = seal(&pt);
    let chunks = len.div_ceil(CS).max(1);
    let expect_len = HEADER_LEN + len + chunks * TAG;
    assert_eq!(file.len(), expect_len, "len {len}");
    assert_eq!(plaintext_len(file.len() as u64, CS as u64).unwrap(), len as u64);
    let mut r = ChunkedReader::open(&s, PATH, Cursor::new(file)).unwrap();
    assert_eq!(r.len(), len as u64);
    assert_eq!(r.chunk_count(), chunks as u64);
    assert_eq!(&r.read_all().unwrap()[..], &pt[..], "len {len}");
  }
}

#[test]
fn round_trip_default_and_extreme_chunk_sizes() {
  let s = sub();
  for log2 in [12u8, 16, 20] {
    let cs = 1usize << log2;
    for len in [0, cs - 1, cs, cs + 1, 2 * cs + 3] {
      let pt = data(len);
      let file = seal_with(&s, PATH, log2, &pt, 1);
      assert_eq!(read(&s, PATH, file).unwrap(), pt, "log2 {log2} len {len}");
    }
  }
}

#[test]
fn byte_at_a_time_writes() {
  let s = sub();
  let pt = data(2 * CS + 5);
  let mut w = ChunkedWriter::new(&s, PATH, LOG2, Vec::new()).unwrap();
  for b in &pt {
    w.write_all(std::slice::from_ref(b)).unwrap();
  }
  let file = w.finish().unwrap();
  // Same length as a bulk write (contents differ: random salt).
  assert_eq!(seal(&pt).len(), file.len());
  assert_eq!(read(&s, PATH, file).unwrap(), pt);
}

#[test]
fn rejects_bad_chunk_sizes() {
  for log2 in [0u8, 11, 21, 64, 255] {
    assert!(ChunkedWriter::new(&sub(), PATH, log2, Vec::new()).is_err(), "{log2}");
  }
}

#[test]
fn random_access_ranges() {
  let s = sub();
  let len = 5 * CS + 7;
  let pt = data(len);
  let mut r = ChunkedReader::open(&s, PATH, Cursor::new(seal(&pt))).unwrap();
  let c = CS as u64;
  let ranges = [
    0..0,
    0..1,
    0..c,
    c - 1..c + 1,
    c..2 * c,
    c - 3..3 * c + 5,
    5 * c..5 * c + 7,
    5 * c + 6..5 * c + 7,
    0..len as u64,
    17..17,
    2 * c..2 * c,
  ];
  for rg in ranges {
    let got = r.read_range(rg.clone()).unwrap();
    assert_eq!(&got[..], &pt[rg.start as usize..rg.end as usize], "{rg:?}");
  }
  for i in 0..r.chunk_count() {
    let got = r.read_chunk(i).unwrap();
    let lo = i as usize * CS;
    assert_eq!(&got[..], &pt[lo..(lo + CS).min(len)]);
  }
  assert!(matches!(r.read_chunk(6), Err(Error::OutOfRange)));
  assert!(matches!(r.read_range(0..len as u64 + 1), Err(Error::OutOfRange)));
  #[allow(clippy::reversed_empty_ranges)]
  let inverted = 5..4;
  assert!(matches!(r.read_range(inverted), Err(Error::OutOfRange)));
}

#[test]
fn empty_file_is_one_empty_final_chunk() {
  let file = seal(b"");
  assert_eq!(file.len(), HEADER_LEN + TAG);
  let mut r = ChunkedReader::open(&sub(), PATH, Cursor::new(file)).unwrap();
  assert!(r.is_empty());
  assert_eq!(r.chunk_count(), 1);
  assert!(r.read_all().unwrap().is_empty());
  assert!(r.read_chunk(0).unwrap().is_empty());
}

#[test]
fn header_layout() {
  let file = seal(b"x");
  assert_eq!(&file[..8], b"SPLCRY01");
  assert_eq!(file[8], 1);
  assert_eq!(file[9], LOG2);
  assert_eq!(&file[10..12], &[0, 0]);
  // Random salt.
  assert_ne!(&seal(b"x")[12..44], &file[12..44]);
}

#[test]
fn flip_any_header_bit_fails() {
  let file = seal(&data(CS + 10));
  for i in 0..HEADER_LEN {
    for bit in 0..8 {
      let mut t = file.clone();
      t[i] ^= 1 << bit;
      assert!(read(&sub(), PATH, t).is_err(), "header byte {i} bit {bit}");
    }
  }
}

#[test]
fn flip_bits_in_every_chunk_ciphertext_and_tag_fails() {
  let pt = data(3 * CS + 100);
  let file = seal(&pt);
  // Positions: first/middle/last ciphertext byte and every tag byte of each chunk.
  for i in 0..4 {
    let start = chunk_off(i);
    let ct_len = if i < 3 { CS } else { 100 };
    let mut positions = vec![start, start + ct_len / 2, start + ct_len - 1];
    positions.extend(start + ct_len..start + ct_len + TAG);
    for p in positions {
      for bit in [0, 7] {
        let mut t = file.clone();
        t[p] ^= 1 << bit;
        assert!(read(&sub(), PATH, t).is_err(), "chunk {i} pos {p} bit {bit}");
      }
    }
  }
  // Sanity: the untouched file still reads.
  assert_eq!(read(&sub(), PATH, file).unwrap(), pt);
}

#[test]
fn flipping_a_chunk_breaks_only_that_chunk_for_random_access() {
  let pt = data(3 * CS);
  let mut file = seal(&pt);
  file[chunk_off(1) + 5] ^= 1;
  let mut r = ChunkedReader::open(&sub(), PATH, Cursor::new(file)).unwrap();
  assert!(r.read_chunk(0).is_ok());
  assert!(matches!(r.read_chunk(1), Err(Error::Auth)));
  assert!(r.read_range(CS as u64 - 1..CS as u64 + 1).is_err());
  assert!(r.read_all().is_err());
}

#[test]
fn truncation_at_chunk_boundaries_fails() {
  let pt = data(4 * CS);
  let file = seal(&pt);
  for n in 1..4 {
    let t = file[..chunk_off(n)].to_vec();
    assert!(plaintext_len(t.len() as u64, CS as u64).is_ok(), "boundary length is plausible");
    assert!(matches!(read(&sub(), PATH, t), Err(Error::Auth)), "truncated to {n} chunks");
  }
  // Header only: no final chunk at all.
  assert!(matches!(read(&sub(), PATH, file[..HEADER_LEN].to_vec()), Err(Error::InvalidLength(_))));
  // Truncation at arbitrary points also fails.
  for cut in [file.len() - 1, file.len() - TAG, chunk_off(2) + 3, HEADER_LEN + 15] {
    assert!(read(&sub(), PATH, file[..cut].to_vec()).is_err(), "cut {cut}");
  }
}

#[test]
fn dropping_the_last_short_chunk_fails() {
  let pt = data(3 * CS + 9);
  let file = seal(&pt);
  let t = file[..chunk_off(3)].to_vec();
  assert!(read(&sub(), PATH, t).is_err());
}

#[test]
fn swapping_chunks_fails() {
  let pt = data(4 * CS);
  let file = seal(&pt);
  let mut t = file.clone();
  let (a, b) = (chunk_off(0), chunk_off(1));
  let c0 = file[a..b].to_vec();
  let c1 = file[b..chunk_off(2)].to_vec();
  t[a..b].copy_from_slice(&c1);
  t[b..chunk_off(2)].copy_from_slice(&c0);
  assert!(read(&sub(), PATH, t).is_err());
  // Swapping the last full chunk with a middle one also fails (at open).
  let mut t = file.clone();
  let c3 = file[chunk_off(3)..].to_vec();
  let c2 = file[chunk_off(2)..chunk_off(3)].to_vec();
  t[chunk_off(2)..chunk_off(3)].copy_from_slice(&c3);
  t[chunk_off(3)..].copy_from_slice(&c2);
  assert!(matches!(
    ChunkedReader::open(&sub(), PATH, Cursor::new(t)).map(|_| ()),
    Err(Error::Auth)
  ));
}

#[test]
fn appending_chunks_fails() {
  let pt = data(2 * CS);
  let file = seal(&pt);
  // Duplicate the final chunk.
  let mut t = file.clone();
  t.extend_from_slice(&file[chunk_off(1)..]);
  assert!(read(&sub(), PATH, t).is_err());
  // Duplicate a middle chunk at the end.
  let mut t = file.clone();
  t.extend_from_slice(&file[chunk_off(0)..chunk_off(1)]);
  assert!(read(&sub(), PATH, t).is_err());
  // Append a final chunk taken from another file under the same key/path.
  let other = seal(&data(10));
  let mut t = file.clone();
  t.extend_from_slice(&other[HEADER_LEN..]);
  assert!(read(&sub(), PATH, t).is_err());
  // Trailing garbage of tag length (would look like an empty final chunk).
  let mut t = file.clone();
  t.extend_from_slice(&[0u8; TAG]);
  assert!(matches!(read(&sub(), PATH, t), Err(Error::InvalidLength(_))));
  // Trailing garbage of more than a tag (a short final chunk).
  let mut t = file;
  t.extend_from_slice(&[0u8; TAG + 5]);
  assert!(matches!(read(&sub(), PATH, t), Err(Error::Auth)));
}

#[test]
fn splicing_chunks_between_files_fails() {
  let pt = data(2 * CS);
  let a = seal(&pt);
  let b = seal(&pt);
  let mut t = a.clone();
  t[chunk_off(0)..chunk_off(1)].copy_from_slice(&b[chunk_off(0)..chunk_off(1)]);
  assert!(read(&sub(), PATH, t).is_err());
}

#[test]
fn wrong_logical_path_fails() {
  let file = seal(&data(100));
  for p in [&b"blobs/0002"[..], b"", b"blobs/000", b"blobs/00010"] {
    assert!(matches!(read(&sub(), p, file.clone()), Err(Error::Auth)), "{p:?}");
  }
}

#[test]
fn wrong_subkey_fails() {
  let file = seal(&data(100));
  let other = DataKey::from_bytes(Zeroizing::new([0x43; 32])).derive(Label::Blob);
  assert!(matches!(read(&other, PATH, file.clone()), Err(Error::Auth)));
  // Same data key, different label.
  let index = DataKey::from_bytes(Zeroizing::new([0x42; 32])).derive(Label::Index);
  assert!(matches!(read(&index, PATH, file), Err(Error::Auth)));
}

#[test]
fn writer_dropped_without_finish_is_unreadable() {
  let s = sub();
  for len in [0, 10, CS, CS + 1, 3 * CS, 3 * CS + 1] {
    let mut out = Vec::new();
    {
      let mut w = ChunkedWriter::new(&s, PATH, LOG2, &mut out).unwrap();
      w.write_all(&data(len)).unwrap();
      w.flush().unwrap();
      // dropped here
    }
    assert!(read(&s, PATH, out).is_err(), "len {len}");
  }
}

#[test]
fn seal_and_open_file() {
  let dir = tempfile::tempdir().unwrap();
  let path = dir.path().join("blob.bin");
  let s = sub();
  let pt = data(3 * 65536 + 11);
  seal_file(&path, &s, PATH, 16, &pt).unwrap();
  assert_eq!(&open_file(&path, &s, PATH).unwrap()[..], &pt[..]);
  // Overwrite atomically.
  seal_file(&path, &s, PATH, 16, b"second").unwrap();
  assert_eq!(&open_file(&path, &s, PATH).unwrap()[..], b"second");
  // No temp files left behind.
  let names: Vec<_> =
    std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
  assert_eq!(names, vec![std::ffi::OsString::from("blob.bin")]);
  // Mode 0600.
  use std::os::unix::fs::PermissionsExt;
  assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
  // Wrong path / key.
  assert!(open_file(&path, &s, b"other").is_err());
  // Bad chunk size leaves nothing behind.
  assert!(seal_file(&dir.path().join("x"), &s, PATH, 9, b"").is_err());
  assert!(!dir.path().join("x").exists());
}

proptest! {
  #![proptest_config(ProptestConfig::with_cases(64))]

  #[test]
  fn prop_round_trip_and_ranges(
    len in 0usize..(4 * CS + 50),
    a in any::<u64>(),
    b in any::<u64>(),
    writes in 1usize..20,
  ) {
    let s = sub();
    let pt = data(len);
    let file = seal_with(&s, PATH, LOG2, &pt, writes);
    prop_assert_eq!(plaintext_len(file.len() as u64, CS as u64).unwrap(), len as u64);
    let mut r = ChunkedReader::open(&s, PATH, Cursor::new(file)).unwrap();
    prop_assert_eq!(&r.read_all().unwrap()[..], &pt[..]);
    let n = len as u64 + 1;
    let (lo, hi) = { let (x, y) = (a % n, b % n); (x.min(y), x.max(y)) };
    prop_assert_eq!(&r.read_range(lo..hi).unwrap()[..], &pt[lo as usize..hi as usize]);
  }

  #[test]
  fn prop_any_single_byte_change_fails(len in 0usize..(3 * CS), pos in any::<usize>(), x in 1u8..=255) {
    let pt = data(len);
    let mut file = seal(&pt);
    let p = pos % file.len();
    file[p] ^= x;
    prop_assert!(read(&sub(), PATH, file).is_err());
  }

  #[test]
  fn prop_plaintext_len_consistent(file_len in 0u64..(1 << 24)) {
    // Either impossible, or exactly the length the writer would produce.
    if let Ok(pt) = plaintext_len(file_len, CS as u64) {
      let chunks = pt.div_ceil(CS as u64).max(1);
      prop_assert_eq!(HEADER_LEN as u64 + pt + chunks * TAG as u64, file_len);
    }
  }
}

/// Exhaustive bijection check for small files: every plaintext length maps to
/// exactly one file length, and every other file length is rejected.
#[test]
fn plaintext_len_is_a_bijection() {
  let file_len_of = |pt: u64| HEADER_LEN as u64 + pt + pt.div_ceil(CS as u64).max(1) * TAG as u64;
  let max_pt = 5 * CS as u64 + 3;
  let mut expected = std::collections::HashMap::new();
  for pt in 0..=max_pt {
    assert!(expected.insert(file_len_of(pt), pt).is_none(), "two plaintexts share a length");
  }
  for file_len in 0..=file_len_of(max_pt) {
    match (plaintext_len(file_len, CS as u64), expected.get(&file_len)) {
      (Ok(pt), Some(want)) => assert_eq!(pt, *want, "file_len {file_len}"),
      (Err(Error::InvalidLength(_)), None) => {}
      (got, want) => panic!("file_len {file_len}: got {got:?}, want {want:?}"),
    }
  }
}
