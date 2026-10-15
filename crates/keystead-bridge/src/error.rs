//! Infrastructure errors of the bridge crate (sockets, files, registry).
//!
//! Errors that travel over the wire to the extension are
//! [`BridgeError`](crate::protocol::BridgeError) codes instead.

use std::io;

/// Error of the bridge infrastructure (not a protocol error).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// File system, socket or registry error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// A frame announced (or would need) more bytes than allowed.
    #[error("message of {len} bytes exceeds the limit of {max} bytes")]
    FrameTooLarge { len: usize, max: usize },
    /// Malformed JSON.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Another live server (normally a second app instance) already listens
    /// on the bridge endpoint. Carries the endpoint for diagnostics.
    #[error("another Keystead instance is already serving the browser bridge at {0}")]
    AlreadyRunning(String),
    /// Error from `keystead-core` (e.g. the OS random number generator).
    #[error(transparent)]
    Core(#[from] keystead_core::Error),
}

/// Result alias of the bridge crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Stable error code in the style of the desktop backend
    /// (`io:<detail>`, `corrupt:<detail>`, ...) for Tauri commands.
    pub fn code(&self) -> String {
        match self {
            Error::Io(e) => format!("io:{e}"),
            Error::FrameTooLarge { .. } => "io:message_too_large".to_owned(),
            Error::Json(e) => format!("corrupt:{e}"),
            Error::AlreadyRunning(_) => "io:bridge_already_running".to_owned(),
            Error::Core(e) => e.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(
            Error::AlreadyRunning("x".into()).code(),
            "io:bridge_already_running"
        );
        assert_eq!(
            Error::FrameTooLarge { len: 9, max: 1 }.code(),
            "io:message_too_large"
        );
        assert!(Error::Io(io::Error::other("boom"))
            .code()
            .starts_with("io:"));
    }
}
