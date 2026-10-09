//! VaultX browser bridge. See "Browser bridge protocol" in
//! docs/ARCHITECTURE.md.
//!
//! * [`protocol`] – request/response types and error codes,
//! * [`framing`] – native-messaging framing (`u32` LE length + JSON),
//! * [`socket`] – the per-user local socket / named pipe,
//! * [`server`] – the server the desktop app runs ([`start_server`]),
//! * [`dispatcher`] – pairing, lock state and rate limiting on top of the
//!   app's [`VaultBackend`],
//! * [`clients`] – paired browser clients (`bridge-clients.json`),
//! * [`host`] – native messaging host mode ([`host::run`]),
//! * [`register`] – host manifest registration per browser.
//!
//! Everything is synchronous (std threads, no async runtime).

pub mod clients;
pub mod dispatcher;
pub mod error;
pub mod framing;
pub mod host;
pub mod protocol;
pub mod register;
pub mod server;
pub mod socket;
mod util;

pub use clients::{ClientStore, PairedClient};
pub use dispatcher::{Dispatcher, DispatcherConfig, PairingRequest, VaultBackend};
pub use error::{Error, Result};
pub use protocol::{BridgeError, LoginSecret, Payload, Request, Response};
pub use register::{BrowserId, BrowserInfo, EXTENSION_ID, HOST_NAME};
pub use server::{start_server, start_server_at, BridgeHandler, ServerHandle};
pub use socket::Endpoint;
