//! `spoolctl`: command-line client for `spoold`.
//!
//! Exit codes: 0 success, 1 error (daemon error/unreachable), 2 usage
//! (clap), 3 nothing to print (`current`: empty history; `pick` / `edit`: cancelled).

mod client;
mod escape;

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use clap::{Parser, Subcommand};
use spool_proto::{MAX_FRAME, PauseState, PublicReq, PublicResp, StatusInfo};

/// Default mime for `copy`.
const DEFAULT_MIME: &str = "text/plain;charset=utf-8";

/// Room left in a frame for the request envelope (variant tag, mime and
/// length prefixes) besides the data itself.
const ENVELOPE_SLACK: usize = 64;

#[derive(Debug, Parser)]
#[command(name = "spoolctl", version, about = "Control the Spool clipboard daemon")]
pub struct Cli {
  #[command(subcommand)]
  pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
  /// Copy FILE (or stdin when omitted or `-`) to the clipboard.
  Copy {
    /// MIME type of the data.
    #[arg(long, default_value = DEFAULT_MIME)]
    mime: String,
    /// Input file; `-` or absent = stdin.
    file: Option<PathBuf>,
  },
  /// Open the picker (for compositors without a built-in shortcut: bind a
  /// key to `spoolctl show`).
  Show,
  /// Open the picker and print the item you choose instead of pasting it
  /// (Esc cancels, exit 3). Waits for your choice.
  Pick {
    /// Write bytes verbatim even to a terminal (default: escape control,
    /// bidi and zero-width characters when stdout is a TTY).
    #[arg(long)]
    raw: bool,
  },
  /// Open the picker and edit the item you choose in your external editor
  /// (`[editor]` in config.toml; Ctrl+Shift+E in the picker picks another
  /// format). Each save is stored as a new item, and the last one goes on
  /// the clipboard when the editor closes; the original is never changed.
  /// Returns once the editor started (Esc cancels, exit 3).
  Edit,
  /// Start writing a new item in your external editor (an empty file).
  /// Returns once the editor started.
  New {
    /// MIME type of the new item.
    #[arg(long, default_value = DEFAULT_MIME)]
    mime: String,
  },
  /// Print the current (newest) clipboard item.
  Current {
    /// Write bytes verbatim even to a terminal (default: escape control,
    /// bidi and zero-width characters when stdout is a TTY).
    #[arg(long)]
    raw: bool,
  },
  /// Show daemon status.
  Status {
    /// Print machine-readable JSON.
    #[arg(long)]
    json: bool,
  },
  /// Pause recording.
  Pause {
    /// Resume automatically after SECS seconds.
    #[arg(long = "for", value_name = "SECS")]
    for_secs: Option<u32>,
  },
  /// Resume recording.
  Resume,
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  match run(cli) {
    Ok(code) => code,
    Err(e) => {
      if is_broken_pipe(&e) {
        return ExitCode::SUCCESS;
      }
      eprintln!("spoolctl: {e:#}");
      ExitCode::from(1)
    }
  }
}

fn is_broken_pipe(e: &anyhow::Error) -> bool {
  e.chain()
    .filter_map(|c| c.downcast_ref::<std::io::Error>())
    .any(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
}

/// Execute one command against the daemon.
fn run(cli: Cli) -> anyhow::Result<ExitCode> {
  // Read input before connecting so a slow stdin does not hold a
  // connection open (the daemon closes idle ones).
  let req = match &cli.command {
    Command::Copy { mime, file } => {
      let data = read_input(file.as_deref())?;
      check_copy_size(mime, data.len())?;
      PublicReq::Copy { mime: mime.clone(), data }
    }
    Command::Current { .. } => PublicReq::Current,
    Command::Show => PublicReq::Show,
    Command::Pick { .. } => PublicReq::Pick,
    Command::Edit => PublicReq::Edit,
    Command::New { mime } => PublicReq::New { mime: mime.clone() },
    Command::Status { .. } => PublicReq::Status,
    Command::Pause { for_secs } => PublicReq::Pause { secs: *for_secs },
    Command::Resume => PublicReq::Resume,
  };
  let mut client = client::Client::connect()?;
  let resp = match req {
    // No reply timeout: the user is choosing.
    PublicReq::Pick | PublicReq::Edit => client.request_untimed(&req)?,
    _ => client.request(&req)?,
  };
  if let PublicResp::Error { code, message } = &resp {
    anyhow::bail!("spoold: {message} ({code:?})");
  }
  if resp == PublicResp::NotYetImplemented {
    anyhow::bail!("spoold has no picker (is spool-picker installed on its PATH?)");
  }
  match cli.command {
    Command::Copy { .. }
    | Command::Resume
    | Command::Pause { .. }
    | Command::Show
    | Command::New { .. } => {
      expect_ok(resp)?;
      Ok(ExitCode::SUCCESS)
    }
    Command::Edit => match resp {
      PublicResp::Ok => Ok(ExitCode::SUCCESS),
      PublicResp::Cancelled => {
        eprintln!("spoolctl: cancelled");
        Ok(ExitCode::from(3))
      }
      other => anyhow::bail!("unexpected response from spoold: {}", resp_name(&other)),
    },
    Command::Current { raw } => match resp {
      PublicResp::Current { mime, data } => {
        print_current(&mime, &data, raw, std::io::stdout().is_terminal())?;
        Ok(ExitCode::SUCCESS)
      }
      PublicResp::Empty => {
        eprintln!("spoolctl: clipboard history is empty");
        Ok(ExitCode::from(3))
      }
      other => anyhow::bail!("unexpected response from spoold: {}", resp_name(&other)),
    },
    Command::Pick { raw } => match resp {
      PublicResp::Picked { mime, data } => {
        print_current(&mime, &data, raw, std::io::stdout().is_terminal())?;
        Ok(ExitCode::SUCCESS)
      }
      PublicResp::Cancelled => {
        eprintln!("spoolctl: cancelled");
        Ok(ExitCode::from(3))
      }
      other => anyhow::bail!("unexpected response from spoold: {}", resp_name(&other)),
    },
    Command::Status { json } => match resp {
      PublicResp::Status(info) => {
        let now = SystemTime::now();
        let text = if json { status_json(&info) } else { status_human(&info, now) };
        let mut out = std::io::stdout().lock();
        out.write_all(text.as_bytes())?;
        out.flush()?;
        Ok(ExitCode::SUCCESS)
      }
      other => anyhow::bail!("unexpected response from spoold: {}", resp_name(&other)),
    },
  }
}

fn expect_ok(resp: PublicResp) -> anyhow::Result<()> {
  match resp {
    PublicResp::Ok => Ok(()),
    other => anyhow::bail!("unexpected response from spoold: {}", resp_name(&other)),
  }
}

fn resp_name(r: &PublicResp) -> &'static str {
  match r {
    PublicResp::Ok => "Ok",
    PublicResp::Current { .. } => "Current",
    PublicResp::Empty => "Empty",
    PublicResp::Status(_) => "Status",
    PublicResp::Error { .. } => "Error",
    PublicResp::NotYetImplemented => "NotYetImplemented",
    PublicResp::Picked { .. } => "Picked",
    PublicResp::Cancelled => "Cancelled",
  }
}

/// Read FILE (or stdin for `None`/`-`), at most one byte past the limit.
fn read_input(file: Option<&Path>) -> anyhow::Result<Vec<u8>> {
  let limit = MAX_FRAME as u64 + 1;
  let mut data = Vec::new();
  match file {
    None => {
      std::io::stdin().lock().take(limit).read_to_end(&mut data).context("reading stdin")?;
    }
    Some(p) if p == Path::new("-") => {
      std::io::stdin().lock().take(limit).read_to_end(&mut data).context("reading stdin")?;
    }
    Some(p) => {
      let f = std::fs::File::open(p).with_context(|| format!("opening {}", p.display()))?;
      f.take(limit).read_to_end(&mut data).with_context(|| format!("reading {}", p.display()))?;
    }
  }
  Ok(data)
}

/// Refuse input that cannot fit in one frame.
fn check_copy_size(mime: &str, len: usize) -> anyhow::Result<()> {
  let max = MAX_FRAME.saturating_sub(mime.len() + ENVELOPE_SLACK);
  if len > max {
    anyhow::bail!(
      "input is too large ({}{} bytes); spoolctl copy accepts at most {max} bytes",
      if len > MAX_FRAME { "more than " } else { "" },
      len.min(MAX_FRAME)
    );
  }
  Ok(())
}

fn is_text_mime(mime: &str) -> bool {
  mime.starts_with("text/") || matches!(mime, "UTF8_STRING" | "TEXT" | "STRING")
}

/// Write the current item: escaped text / a placeholder for binary data on a
/// TTY (unless `raw`), verbatim bytes otherwise.
fn print_current(mime: &str, data: &[u8], raw: bool, tty: bool) -> std::io::Result<()> {
  let mut out = std::io::stdout().lock();
  out.write_all(&render_current(mime, data, raw, tty))?;
  out.flush()
}

fn render_current(mime: &str, data: &[u8], raw: bool, tty: bool) -> Vec<u8> {
  if raw || !tty {
    return data.to_vec();
  }
  if !is_text_mime(mime) {
    return format!("[{} {} bytes]\n", escape::escape_for_tty(mime.as_bytes()), data.len())
      .into_bytes();
  }
  let mut s = escape::escape_for_tty(data);
  if !s.ends_with('\n') {
    s.push('\n');
  }
  s.into_bytes()
}

fn unix_ms_to_system(ms: u64) -> SystemTime {
  SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
}

fn status_human(info: &StatusInfo, now: SystemTime) -> String {
  let paused = match info.paused {
    PauseState::Recording => "no (recording)".to_string(),
    PauseState::PausedIndefinitely => "yes (until resumed)".to_string(),
    PauseState::PausedUntil { unix_ms } => {
      let left = unix_ms_to_system(unix_ms).duration_since(now).unwrap_or_default().as_secs();
      format!("yes ({left} s left)")
    }
  };
  let yn = |b: bool| if b { "yes" } else { "no" };
  format!(
    "spoold {}\npaused:     {}\nitems:      {}\ncompositor: {}\nprimary:    {}\nencrypted:  {}\nunlocked:   {}\nkey:        {}\n",
    info.version,
    paused,
    info.item_count,
    info.compositor.as_deref().unwrap_or("(not connected)"),
    yn(info.primary_enabled),
    yn(info.encrypted),
    yn(info.unlocked),
    escape::escape_for_tty(info.key_state.as_bytes()),
  ) + &match &info.capabilities {
    Some(c) => {
      escape::escape_for_tty(
        format!(
          "desktop:    {} (shortcut: {}, focus: {}, cursor: {}, auto-paste: {} via {})",
          c.compositor,
          c.hotkey,
          c.focus,
          yn(c.cursor),
          yn(c.auto_paste),
          c.paste_backend
        )
        .as_bytes(),
      ) + "\n"
    }
    None => String::new(),
  }
}

fn json_str(s: &str) -> String {
  let mut out = String::with_capacity(s.len() + 2);
  out.push('"');
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
      c => out.push(c),
    }
  }
  out.push('"');
  out
}

fn status_json(info: &StatusInfo) -> String {
  let paused = match info.paused {
    PauseState::Recording => r#"{"state":"recording"}"#.to_string(),
    PauseState::PausedIndefinitely => r#"{"state":"paused"}"#.to_string(),
    PauseState::PausedUntil { unix_ms } => {
      format!(r#"{{"state":"paused","until_unix_ms":{unix_ms}}}"#)
    }
  };
  format!(
    "{{\"version\":{},\"paused\":{},\"item_count\":{},\"compositor\":{},\"primary_enabled\":{},\"encrypted\":{},\"unlocked\":{},\"key_state\":{}}}\n",
    json_str(&info.version),
    paused,
    info.item_count,
    info.compositor.as_deref().map(json_str).unwrap_or_else(|| "null".into()),
    info.primary_enabled,
    info.encrypted,
    info.unlocked,
    json_str(&info.key_state),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cli_parses() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
    let c = Cli::try_parse_from(["spoolctl", "pause", "--for", "30"]).unwrap();
    assert!(matches!(c.command, Command::Pause { for_secs: Some(30) }));
    let c = Cli::try_parse_from(["spoolctl", "copy", "--mime", "image/png", "-"]).unwrap();
    assert!(matches!(c.command, Command::Copy { .. }));
    let c = Cli::try_parse_from(["spoolctl", "copy"]).unwrap();
    assert!(matches!(c.command, Command::Copy { ref mime, file: None } if mime == DEFAULT_MIME));
    let c = Cli::try_parse_from(["spoolctl", "pick", "--raw"]).unwrap();
    assert!(matches!(c.command, Command::Pick { raw: true }));
    let c = Cli::try_parse_from(["spoolctl", "show"]).unwrap();
    assert!(matches!(c.command, Command::Show));
    let c = Cli::try_parse_from(["spoolctl", "edit"]).unwrap();
    assert!(matches!(c.command, Command::Edit));
    // There is deliberately no way to name an item: the public socket
    // cannot read history, the user picks it in the picker.
    assert!(Cli::try_parse_from(["spoolctl", "edit", "12"]).is_err());
    assert!(Cli::try_parse_from(["spoolctl", "edit", "--id", "12"]).is_err());
    let c = Cli::try_parse_from(["spoolctl", "new"]).unwrap();
    assert!(matches!(c.command, Command::New { ref mime } if mime == DEFAULT_MIME));
    let c = Cli::try_parse_from(["spoolctl", "new", "--mime", "text/html"]).unwrap();
    assert!(matches!(c.command, Command::New { ref mime } if mime == "text/html"));
    assert!(Cli::try_parse_from(["spoolctl", "new", "file.txt"]).is_err());
    let c = Cli::try_parse_from(["spoolctl", "status", "--json"]).unwrap();
    assert!(matches!(c.command, Command::Status { json: true }));
    assert!(Cli::try_parse_from(["spoolctl", "pause", "--for", "x"]).is_err());
  }

  #[test]
  fn copy_size_limit() {
    assert!(check_copy_size(DEFAULT_MIME, 10).is_ok());
    assert!(check_copy_size(DEFAULT_MIME, MAX_FRAME - 200).is_ok());
    let e = check_copy_size(DEFAULT_MIME, MAX_FRAME + 1).unwrap_err().to_string();
    assert!(e.contains("too large"), "{e}");
    // Anything accepted must also encode into one frame.
    let max = MAX_FRAME - DEFAULT_MIME.len() - ENVELOPE_SLACK;
    let req = PublicReq::Copy { mime: DEFAULT_MIME.into(), data: vec![0; max] };
    assert!(spool_proto::encode_frame(&req).is_ok());
  }

  #[test]
  fn read_input_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("in");
    std::fs::write(&p, b"abc").unwrap();
    assert_eq!(read_input(Some(&p)).unwrap(), b"abc");
    assert!(read_input(Some(&d.path().join("missing"))).is_err());
  }

  #[test]
  fn current_rendering() {
    let m = DEFAULT_MIME;
    assert_eq!(render_current(m, b"a\x1bb", false, true), b"a\\x1bb\n");
    assert_eq!(render_current(m, b"a\x1bb", true, true), b"a\x1bb");
    assert_eq!(render_current(m, b"a\x1bb", false, false), b"a\x1bb");
    assert_eq!(render_current(m, b"line\n", false, true), b"line\n");
    assert_eq!(render_current("image/png", &[0, 1, 2], false, true), b"[image/png 3 bytes]\n");
    assert_eq!(render_current("image/png", &[0, 1, 2], false, false), &[0, 1, 2]);
    assert_eq!(render_current("image/png", &[0, 1, 2], true, true), &[0, 1, 2]);
  }

  #[test]
  fn status_output() {
    let info = StatusInfo {
      version: "0.1.0".into(),
      paused: PauseState::PausedUntil { unix_ms: 10_000 },
      item_count: 7,
      compositor: Some("wayland-1 (\"x\")".into()),
      primary_enabled: false,
      encrypted: false,
      unlocked: true,
      key_state: "error: \"x\"".into(),
      capabilities: None,
    };
    let h = status_human(&info, unix_ms_to_system(4_000));
    assert!(h.contains("6 s left") && h.contains("items:      7"), "{h}");
    assert!(h.contains("key:        error: \"x\""), "{h}");
    let j = status_json(&info);
    assert_eq!(
      j,
      "{\"version\":\"0.1.0\",\"paused\":{\"state\":\"paused\",\"until_unix_ms\":10000},\"item_count\":7,\"compositor\":\"wayland-1 (\\\"x\\\")\",\"primary_enabled\":false,\"encrypted\":false,\"unlocked\":true,\"key_state\":\"error: \\\"x\\\"\"}\n"
    );
    let none = StatusInfo { compositor: None, paused: PauseState::Recording, ..info };
    assert!(status_json(&none).contains("\"compositor\":null"));
    assert!(status_human(&none, SystemTime::now()).contains("not connected"));
  }
}
