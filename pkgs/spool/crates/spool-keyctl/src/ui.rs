//! Terminal interaction. Everything goes through the [`Ui`] trait so tests
//! can script answers.
//!
//! Results go to stdout ([`Ui::out`]); progress and warnings to stderr
//! ([`Ui::note`]); prompts and typed input use the controlling terminal
//! (`/dev/tty`), never stdin/stdout, so secrets cannot end up in a pipe.

use std::fs::File;
use std::io::{Read as _, Write as _};

use rustix::termios::{
  InputModes, LocalModes, OptionalActions, SpecialCodeIndex, Termios, tcgetattr, tcsetattr,
};
use zeroize::Zeroizing;

/// The user aborted a prompt (Ctrl-C / Ctrl-D) or no terminal is available.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
  /// Ctrl-C, or Ctrl-D on an empty line.
  #[error("aborted")]
  Aborted,
  /// No controlling terminal (or another I/O error).
  #[error("terminal: {0}")]
  Io(#[from] std::io::Error),
}

/// User interaction.
pub trait Ui: Sync {
  /// A line of result output (stdout).
  fn out(&self, line: &str);
  /// Progress / warnings (stderr).
  fn note(&self, line: &str);
  /// Read a secret without echo. Zeroized on drop.
  fn secret(&self, prompt: &str) -> Result<Zeroizing<String>, PromptError>;
  /// Read a visible line (confirmations, choices). Trimmed.
  fn line(&self, prompt: &str) -> Result<String, PromptError>;
}

/// Maximum accepted secret length in bytes (the buffer never reallocates).
pub const MAX_SECRET_LEN: usize = 1024;

/// The real terminal.
#[derive(Debug, Default)]
pub struct TtyUi;

fn open_tty() -> std::io::Result<File> {
  std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").map_err(|e| {
    std::io::Error::new(
      e.kind(),
      format!("no terminal (/dev/tty: {e}); spool-keyctl is interactive"),
    )
  })
}

/// Restores the terminal mode on drop.
struct RawGuard<'a> {
  tty: &'a File,
  orig: Termios,
}

impl Drop for RawGuard<'_> {
  fn drop(&mut self) {
    let _ = tcsetattr(self.tty, OptionalActions::Now, &self.orig);
  }
}

impl Ui for TtyUi {
  fn out(&self, line: &str) {
    println!("{line}");
  }

  fn note(&self, line: &str) {
    eprintln!("{line}");
  }

  fn secret(&self, prompt: &str) -> Result<Zeroizing<String>, PromptError> {
    let mut tty = open_tty()?;
    tty.write_all(prompt.as_bytes())?;
    tty.flush()?;
    let orig = tcgetattr(&tty).map_err(std::io::Error::from)?;
    let mut raw = orig.clone();
    // No echo, byte at a time, and Ctrl-C / Ctrl-D handled here, so an
    // interrupted prompt never leaves the terminal without echo.
    raw.local_modes.remove(LocalModes::ECHO | LocalModes::ICANON | LocalModes::ISIG);
    raw.input_modes.remove(InputModes::IXON);
    raw.input_modes.insert(InputModes::ICRNL);
    raw.special_codes[SpecialCodeIndex::VMIN] = 1;
    raw.special_codes[SpecialCodeIndex::VTIME] = 0;
    tcsetattr(&tty, OptionalActions::Flush, &raw).map_err(std::io::Error::from)?;
    let guard = RawGuard { tty: &tty, orig };

    let mut buf = Zeroizing::new(Vec::with_capacity(MAX_SECRET_LEN));
    let mut byte = [0u8; 1];
    let result = loop {
      match (&tty).read(&mut byte) {
        Ok(0) => break Err(PromptError::Aborted),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(e) => break Err(e.into()),
      }
      match byte[0] {
        b'\n' | b'\r' => break Ok(()),
        0x03 => break Err(PromptError::Aborted),
        0x04 if buf.is_empty() => break Err(PromptError::Aborted),
        0x7f | 0x08 => {
          // Drop one whole UTF-8 character.
          while let Some(b) = buf.pop() {
            if b & 0xC0 != 0x80 {
              break;
            }
          }
        }
        0x15 => buf.clear(),
        b if buf.len() < MAX_SECRET_LEN => buf.push(b),
        _ => {}
      }
    };
    zeroize::Zeroize::zeroize(&mut byte);
    drop(guard);
    let _ = (&tty).write_all(b"\n");
    result?;
    if buf.len() >= MAX_SECRET_LEN {
      return Err(PromptError::Io(std::io::Error::other(format!(
        "input longer than {MAX_SECRET_LEN} bytes"
      ))));
    }
    let bytes = std::mem::take(&mut *buf);
    match String::from_utf8(bytes) {
      Ok(s) => Ok(Zeroizing::new(s)),
      Err(e) => {
        drop(Zeroizing::new(e.into_bytes()));
        Err(PromptError::Io(std::io::Error::other("input is not valid UTF-8")))
      }
    }
  }

  fn line(&self, prompt: &str) -> Result<String, PromptError> {
    let mut tty = open_tty()?;
    tty.write_all(prompt.as_bytes())?;
    tty.flush()?;
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    loop {
      match tty.read(&mut byte) {
        Ok(0) => return Err(PromptError::Aborted),
        Ok(_) if byte[0] == b'\n' => break,
        Ok(_) => out.push(byte[0]),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(e) => return Err(e.into()),
      }
    }
    Ok(String::from_utf8_lossy(&out).trim().to_string())
  }
}

/// Ask the user to type `word` exactly. `Ok(false)` on anything else.
pub fn confirm_word(ui: &dyn Ui, word: &str) -> Result<bool, PromptError> {
  Ok(ui.line(&format!("Type '{word}' to continue: "))? == word)
}

/// y/N question.
pub fn confirm_yes(ui: &dyn Ui, question: &str) -> Result<bool, PromptError> {
  let a = ui.line(&format!("{question} [y/N] "))?;
  Ok(matches!(a.to_ascii_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Manual: needs a terminal. Run under a pty with delayed input, e.g.
  /// `(sleep 2; printf 'p\xc3\xa4ssX\x7f\rok\n') | script -qec "cargo test -p spool-keyctl --lib tty_ -- --ignored --nocapture" /dev/null`
  #[test]
  #[ignore]
  fn tty_secret_and_line() {
    let s = TtyUi.secret("secret: ").unwrap();
    assert_eq!(s.as_str(), "päss", "backspace must drop the X");
    assert_eq!(TtyUi.line("line: ").unwrap(), "ok");
    // Echo is back on after the secret prompt.
    let t = tcgetattr(open_tty().unwrap()).unwrap();
    assert!(t.local_modes.contains(LocalModes::ECHO | LocalModes::ICANON | LocalModes::ISIG));
  }
}
