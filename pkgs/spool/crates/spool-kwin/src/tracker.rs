//! Latest active window as reported by the KWin script.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

/// One `ActiveWindow` report (already validated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveWindow {
  pub app_id: Option<String>,
  pub window_id: Option<String>,
  pub at: Instant,
}

/// Cheaply clonable handle on the most recent active window.
///
/// The [`KwinService`](crate::KwinService) updates it on every
/// `ActiveWindow` call, independently of whether anyone drains the event
/// channel.
#[derive(Debug, Clone)]
pub struct ActiveWindowTracker {
  tx: Arc<watch::Sender<Option<ActiveWindow>>>,
}

impl Default for ActiveWindowTracker {
  fn default() -> Self {
    Self::new()
  }
}

impl ActiveWindowTracker {
  pub fn new() -> Self {
    Self { tx: Arc::new(watch::Sender::new(None)) }
  }

  /// Records a focus change.
  pub fn update(&self, app_id: Option<String>, window_id: Option<String>, at: Instant) {
    self.tx.send_replace(Some(ActiveWindow { app_id, window_id, at }));
  }

  /// The latest report, if any arrived yet.
  pub fn current(&self) -> Option<ActiveWindow> {
    self.tx.borrow().clone()
  }

  /// App id of the active window, for source-app attribution.
  pub fn current_app_id(&self) -> Option<String> {
    self.tx.borrow().as_ref().and_then(|w| w.app_id.clone())
  }

  /// Whether `window_id` is the active window right now.
  pub fn is_active(&self, window_id: &str) -> bool {
    matches!(&*self.tx.borrow(), Some(w) if w.window_id.as_deref() == Some(window_id))
  }

  /// Waits until `window_id` is the active window, for at most `timeout`.
  /// Returns `true` as soon as it is (immediately if it already is), `false`
  /// on timeout. Used by auto-paste: after the picker closes, wait up to
  /// ~500 ms for focus to return to the window the paste is meant for.
  pub async fn wait_for(&self, window_id: &str, timeout: Duration) -> bool {
    let mut rx = self.tx.subscribe();
    let check =
      |w: &Option<ActiveWindow>| matches!(w, Some(w) if w.window_id.as_deref() == Some(window_id));
    let fut = async {
      loop {
        if check(&rx.borrow_and_update()) {
          return true;
        }
        if rx.changed().await.is_err() {
          // Cannot happen while `self` holds the sender; be safe anyway.
          return false;
        }
      }
    };
    tokio::time::timeout(timeout, fut).await.unwrap_or(false)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const A: &str = "0c8f9a0e-3b9c-4d4c-8a8e-2f1d7a3b5c6d";
  const B: &str = "11111111-2222-3333-4444-555555555555";

  #[tokio::test]
  async fn current_and_is_active() {
    let t = ActiveWindowTracker::new();
    assert!(t.current().is_none());
    assert!(!t.is_active(A));
    t.update(Some("org.kde.konsole".into()), Some(A.into()), Instant::now());
    assert!(t.is_active(A));
    assert!(!t.is_active(B));
    assert_eq!(t.current_app_id().as_deref(), Some("org.kde.konsole"));
    t.update(None, None, Instant::now());
    assert!(!t.is_active(A));
    assert_eq!(t.current_app_id(), None);
  }

  #[tokio::test(start_paused = true)]
  async fn wait_for_already_active_is_immediate() {
    let t = ActiveWindowTracker::new();
    t.update(None, Some(A.into()), Instant::now());
    let start = tokio::time::Instant::now();
    assert!(t.wait_for(A, Duration::from_millis(500)).await);
    assert_eq!(start.elapsed(), Duration::ZERO);
  }

  #[tokio::test(start_paused = true)]
  async fn wait_for_times_out() {
    let t = ActiveWindowTracker::new();
    t.update(None, Some(B.into()), Instant::now());
    let start = tokio::time::Instant::now();
    assert!(!t.wait_for(A, Duration::from_millis(500)).await);
    assert_eq!(start.elapsed(), Duration::from_millis(500));
  }

  #[tokio::test(start_paused = true)]
  async fn wait_for_wakes_on_focus_return() {
    let t = ActiveWindowTracker::new();
    t.update(Some("picker".into()), Some(B.into()), Instant::now());
    let t2 = t.clone();
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(50)).await;
      t2.update(Some("x".into()), None, Instant::now());
      tokio::time::sleep(Duration::from_millis(70)).await;
      t2.update(Some("org.kde.kate".into()), Some(A.into()), Instant::now());
    });
    let start = tokio::time::Instant::now();
    assert!(t.wait_for(A, Duration::from_millis(500)).await);
    assert_eq!(start.elapsed(), Duration::from_millis(120));
  }

  #[tokio::test(start_paused = true)]
  async fn wait_for_ignores_other_windows() {
    let t = ActiveWindowTracker::new();
    let t2 = t.clone();
    tokio::spawn(async move {
      for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(40)).await;
        t2.update(None, Some(B.into()), Instant::now());
      }
    });
    assert!(!t.wait_for(A, Duration::from_millis(300)).await);
  }
}
