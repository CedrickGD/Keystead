//! The phone's autofill (Block 5 stage 3, "Autofill (phone)" in
//! docs/ARCHITECTURE.md) – the platform-neutral part, unit-tested on the
//! desktop host. Android's autofill framework asks Keystead's
//! `KeysteadAutofillService` (Kotlin) to fill a login form; Kotlin flattens
//! the form into a [`request::FillRequest`] (no values, no texts) and asks
//! this module, through `mobile::autofill_jni`, what to offer:
//!
//! * [`classify`]: which field is the username and which the password;
//! * [`target`]: whose login it is – a web page in a browser whose signer
//!   is pinned ([`browsers`]), or an app by its package – and whether to
//!   offer anything at all.
//!
//! Never Tauri, never the app's `Core`: the service and its activity may run
//! without either. Nothing here logs or writes.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

pub mod browsers;
pub mod classify;
pub mod request;
pub mod target;

#[cfg(test)]
mod tests;
