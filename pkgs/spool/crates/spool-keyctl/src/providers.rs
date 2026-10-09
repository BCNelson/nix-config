//! Where provider instances come from. Production uses the real Secret
//! Service (prompting allowed: the user is at a terminal), Argon2id
//! passphrases and libfido2; tests substitute fakes.

use async_trait::async_trait;
use spool_keys::{
  Fido2Device, Fido2Provider, KeyProvider, PassphraseProvider, SecretServiceProvider,
  SessionProvider,
};
use zeroize::Zeroizing;

/// Options for a FIDO2 provider.
#[derive(Debug, Clone, Default)]
pub struct Fido2Opts {
  /// PIN typed by the user, if any.
  pub pin: Option<Zeroizing<String>>,
  /// Enroll with user verification.
  pub uv: bool,
  /// Enroll on this hidraw device.
  pub device: Option<String>,
}

/// Factory for provider instances.
#[async_trait]
pub trait Providers: Send + Sync {
  /// Secret Service provider (may show the wallet's unlock dialog).
  fn secret_service(&self) -> Box<dyn KeyProvider>;
  /// Passphrase provider (`None`: only good for `destroy`).
  fn passphrase(&self, passphrase: Option<Zeroizing<String>>) -> Box<dyn KeyProvider>;
  /// FIDO2 provider.
  fn fido2(&self, opts: Fido2Opts) -> Box<dyn KeyProvider>;
  /// Connected security keys.
  async fn fido2_devices(&self) -> spool_keys::Result<Vec<Fido2Device>>;
  /// Delete every Spool item from the Secret Service (orphan cleanup).
  async fn secret_service_destroy_all(&self) -> spool_keys::Result<usize>;
  /// One provider per kind that can `destroy` any slot without user input
  /// (passphrase and FIDO2 destroys are no-ops; session slots only exist
  /// in memory).
  fn destroyers(&self) -> Vec<Box<dyn KeyProvider>> {
    vec![
      self.secret_service(),
      self.passphrase(None),
      self.fido2(Fido2Opts::default()),
      Box::new(SessionProvider::new()),
    ]
  }
}

/// The real providers.
#[derive(Debug, Default)]
pub struct SystemProviders;

#[async_trait]
impl Providers for SystemProviders {
  fn secret_service(&self) -> Box<dyn KeyProvider> {
    Box::new(SecretServiceProvider::new(true))
  }

  fn passphrase(&self, passphrase: Option<Zeroizing<String>>) -> Box<dyn KeyProvider> {
    Box::new(match passphrase {
      Some(p) => PassphraseProvider::new(p),
      None => PassphraseProvider::without_passphrase(),
    })
  }

  fn fido2(&self, opts: Fido2Opts) -> Box<dyn KeyProvider> {
    let mut p = Fido2Provider::new(opts.pin).require_uv(opts.uv);
    if let Some(d) = opts.device {
      p = p.with_device_path(d);
    }
    Box::new(p)
  }

  async fn fido2_devices(&self) -> spool_keys::Result<Vec<Fido2Device>> {
    Fido2Provider::new(None).devices().await
  }

  async fn secret_service_destroy_all(&self) -> spool_keys::Result<usize> {
    SecretServiceProvider::new(true).destroy_all().await
  }
}

/// `&[&dyn KeyProvider]` view of boxed providers.
pub fn refs(v: &[Box<dyn KeyProvider>]) -> Vec<&dyn KeyProvider> {
  v.iter().map(|b| b.as_ref()).collect()
}
