//! VaultX core library: crypto, vault file format, storage, data model,
//! generator, TOTP, URL matching, health report, import/export, settings and
//! clipboard. No UI and no IPC. See docs/ARCHITECTURE.md for the contract.

pub mod clipboard;
pub mod crypto;
pub mod error;
pub mod export;
pub mod format;
pub mod generator;
pub mod health;
pub mod import;
pub mod matching;
pub mod model;
pub mod paths;
pub mod settings;
pub mod store;
pub mod totp;
mod util;
pub mod vault;

pub use crypto::KdfParams;
pub use error::{Error, Result};
pub use store::VaultStore;
pub use vault::UnlockedVault;
