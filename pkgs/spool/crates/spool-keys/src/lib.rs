//! Spool encryption key slots.
//!
//! Clipboard history is encrypted with one random 256-bit **data key**.
//! `keyslots.json` (next to the history DB, `$XDG_STATE_HOME/spool/`, mode
//! 0600) holds one *slot* per unlock method, LUKS-style: provider kind,
//! provider params and the data key sealed with XChaCha20-Poly1305 under that
//! provider's key-encryption key (KEK). See [`wrap`] for the exact sealing
//! and AAD, and [`KeySlots`] for the API.
//!
//! ```json
//! {"version":1,"data_key_id":"<uuid>","slots":[
//!   {"id":"<uuid>","provider":"secret-service",
//!    "params":{"attributes":{"application":"spool","slot":"<uuid>"}},
//!    "wrapped":"<base64 nonce||ciphertext||tag>"}]}
//! ```
//!
//! Providers: [`SecretServiceProvider`] (KWallet 6 / gnome-keyring via oo7),
//! [`SessionProvider`] (memory only), [`PassphraseProvider`] (Argon2id) and
//! [`Fido2Provider`] (FIDO2 `hmac-secret`, libfido2). `tpm2` exists as a
//! [`ProviderKind`] but is not implemented yet ([`Error::NotImplemented`]).
//!
//! Interactive providers (passphrase, FIDO2) get their secret from the
//! caller: construct them with the passphrase / PIN the user typed. Without
//! one they fail with the retryable [`Error::NeedsSecret`], and
//! [`Error::needs_secret`] / [`Error::secret_kinds`] on a
//! [`Error::NoSlotUnlocked`] tell the caller which prompt to show.
//!
//! Nothing in this crate logs or formats key material.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
pub mod fido2;
mod fsutil;
pub mod passphrase;
mod provider;
mod secret_service;
mod session;
mod slots;
pub mod wrap;

pub use error::{Error, Result, SlotAttempt};
pub use fido2::{Fido2Device, Fido2Params, Fido2Provider};
pub use passphrase::{PassphraseParams, PassphraseProvider};
pub use provider::{Kek, KeyProvider, ProviderKind, SlotParams};
pub use secret_service::{
  APPLICATION, COLLECTION_ALIAS, ITEM_LABEL, SecretServiceProvider, slot_attributes,
};
pub use session::SessionProvider;
pub use slots::{
  DataKey, DataKeyId, FILE_NAME, FORMAT_VERSION, KeySlots, MAX_FILE_SIZE, MAX_SLOTS, Slot, SlotId,
};
