//! Spool core: item model, ingest policy, storage and configuration.
//!
//! This crate has no Wayland dependency and no async runtime; everything here
//! is synchronous and unit-testable. The daemon (`spoold`) owns a
//! [`store::Store`], a [`policy::Policy`] and a [`policy::PolicyState`] and
//! drives them from Wayland events.
//!
//! Logging rule (crate-wide): never log clipboard content. Log only hash
//! prefixes ([`item::hash_prefix`]), byte lengths and MIME types.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod item;
pub mod policy;
pub mod store;

pub use error::{Error, Result};
