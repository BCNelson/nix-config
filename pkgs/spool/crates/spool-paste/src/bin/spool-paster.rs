//! `spool-paster`: auto-paste helper for spoold (see `spool_paste::remote`).
//!
//! Started by spoold with a socketpair as stdin and a scrubbed environment.
//! Holds no secrets and stays dumpable, so KWin can match its
//! `/proc/<pid>/exe` against `dev.bcnelson.spool.paster.desktop` and grant
//! it `org_kde_kwin_fake_input`. It only presses the paste chords spoold
//! asks for; it never sees clipboard content.
#![forbid(unsafe_code)]

use std::os::fd::AsFd;
use std::time::Duration;

use spool_paste::PasteConfig;

fn main() {
  let mut cfg = PasteConfig::default();
  let mut args = std::env::args().skip(1);
  while let Some(a) = args.next() {
    match a.as_str() {
      "--spacing-ms" => {
        let ms = args.next().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms <= 1000);
        let Some(ms) = ms else {
          eprintln!("spool-paster: --spacing-ms takes 0..=1000");
          std::process::exit(2);
        };
        cfg.spacing = Duration::from_millis(ms);
      }
      _ => {
        eprintln!("spool-paster: internal helper of spoold; not meant to be run by hand");
        std::process::exit(2);
      }
    }
  }
  let sock = match std::io::stdin().as_fd().try_clone_to_owned() {
    Ok(fd) => std::os::unix::net::UnixStream::from(fd),
    Err(e) => {
      eprintln!("spool-paster: stdin: {e}");
      std::process::exit(2);
    }
  };
  // Not a socket (run from a terminal): refuse instead of reading keys.
  if sock.peer_addr().is_err() {
    eprintln!("spool-paster: stdin is not a socket; this is an internal helper of spoold");
    std::process::exit(2);
  }
  std::process::exit(spool_paste::remote::serve_helper(sock, &cfg));
}
