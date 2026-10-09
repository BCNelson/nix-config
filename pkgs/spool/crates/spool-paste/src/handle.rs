//! Async front-end: a dedicated thread owning the [`Paster`].

use std::sync::mpsc as std_mpsc;

use tokio::sync::oneshot;

use crate::chord::PasteChord;
use crate::paster::{PasteBackend, PasteConfig, Paster};
use crate::{KeyCodes, PasteError};

enum Cmd {
  Paste { chord: PasteChord, layout: Option<u32>, reply: oneshot::Sender<Result<(), PasteError>> },
  Keycodes { layout: Option<u32>, reply: oneshot::Sender<KeyCodes> },
}

/// Clonable handle on the paste thread. The thread exits when the last
/// handle is dropped. If the Wayland connection breaks (compositor restart),
/// the next paste reconnects once.
#[derive(Clone)]
pub struct PasteHandle {
  tx: std_mpsc::Sender<Cmd>,
  backend: PasteBackend,
}

impl PasteHandle {
  /// Connects on a new thread; connection errors (including
  /// [`PasteError::Unsupported`]) are returned synchronously.
  pub fn spawn(cfg: PasteConfig) -> Result<Self, PasteError> {
    let (tx, rx) = std_mpsc::channel::<Cmd>();
    let (ready_tx, ready_rx) = std_mpsc::sync_channel::<Result<PasteBackend, PasteError>>(1);
    std::thread::Builder::new()
      .name("spool-paste".into())
      .spawn(move || {
        let mut paster = match Paster::connect(&cfg) {
          Ok(p) => {
            let _ = ready_tx.send(Ok(p.backend()));
            Some(p)
          }
          Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
          }
        };
        while let Ok(cmd) = rx.recv() {
          match cmd {
            Cmd::Paste { chord, layout, reply } => {
              let mut res = match paster.as_mut() {
                Some(p) => p.paste(chord, layout),
                None => Err(PasteError::Wayland("not connected".into())),
              };
              if matches!(res, Err(PasteError::Wayland(_))) {
                tracing::info!("paste connection lost; reconnecting");
                drop(paster.take()); // close the dead connection first
                paster = Paster::connect(&cfg).ok();
                if let Some(p) = paster.as_mut() {
                  res = p.paste(chord, layout);
                }
              }
              let _ = reply.send(res);
            }
            Cmd::Keycodes { layout, reply } => {
              let k = paster.as_ref().map(|p| p.keycodes(layout)).unwrap_or(KeyCodes::FALLBACK);
              let _ = reply.send(k);
            }
          }
        }
      })
      .map_err(|e| PasteError::Connect(e.to_string()))?;
    let backend = ready_rx.recv().map_err(|_| PasteError::ThreadGone)??;
    Ok(Self { tx, backend })
  }

  /// The protocol chosen at connect time (fake input on KWin, virtual
  /// keyboard on wlroots/Hyprland).
  pub fn backend(&self) -> PasteBackend {
    self.backend
  }

  /// Sends `chord` to the focused window. See [`Paster::paste`].
  pub async fn paste(&self, chord: PasteChord, layout: Option<u32>) -> Result<(), PasteError> {
    let (reply, rx) = oneshot::channel();
    self.tx.send(Cmd::Paste { chord, layout, reply }).map_err(|_| PasteError::ThreadGone)?;
    rx.await.map_err(|_| PasteError::ThreadGone)?
  }

  /// Keycodes the paste thread currently resolves (diagnostics/tests).
  pub async fn keycodes(&self, layout: Option<u32>) -> Result<KeyCodes, PasteError> {
    let (reply, rx) = oneshot::channel();
    self.tx.send(Cmd::Keycodes { layout, reply }).map_err(|_| PasteError::ThreadGone)?;
    rx.await.map_err(|_| PasteError::ThreadGone)
  }
}
