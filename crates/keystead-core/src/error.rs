//! Error type shared by all core modules.
//!
//! [`Error::code`] maps every error to the stable error code the frontends
//! translate (see "Desktop backend ↔ frontend" in docs/ARCHITECTURE.md).
//! Error messages never contain secrets.

use std::path::Path;

/// Core error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// File system or OS error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed JSON in a file that should be well-formed.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Master password, recovery key or import password is wrong.
    #[error("wrong password")]
    WrongPassword,
    /// The data failed an integrity check or cannot be decoded.
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// The vault file was modified by another process since it was loaded.
    #[error("the vault was changed by another process")]
    Conflict,
    /// A vault, item, folder or file does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The caller supplied an invalid argument.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// A feature or file variant that is not supported.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Result alias used throughout the core.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// The stable error code for the UI:
    /// `wrong_password`, `not_found`, `conflict`, `invalid_input:<detail>`,
    /// `io:<detail>`, `corrupt:<detail>` or `unsupported:<detail>`.
    pub fn code(&self) -> String {
        match self {
            Error::Io(e) => format!("io:{e}"),
            Error::Json(e) => format!("corrupt:{e}"),
            Error::WrongPassword => "wrong_password".to_owned(),
            Error::Corrupt(d) => format!("corrupt:{d}"),
            Error::Conflict => "conflict".to_owned(),
            Error::NotFound(_) => "not_found".to_owned(),
            Error::InvalidInput(d) => format!("invalid_input:{d}"),
            Error::Unsupported(d) => format!("unsupported:{d}"),
        }
    }

    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        Error::InvalidInput(detail.into())
    }

    pub(crate) fn corrupt(detail: impl Into<String>) -> Self {
        Error::Corrupt(detail.into())
    }

    /// Wraps an I/O error so that the message names the affected path.
    pub(crate) fn io_at(path: &Path, err: std::io::Error) -> Self {
        Error::Io(std::io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(Error::WrongPassword.code(), "wrong_password");
        assert_eq!(Error::Conflict.code(), "conflict");
        assert_eq!(Error::NotFound("x".into()).code(), "not_found");
        assert_eq!(
            Error::InvalidInput("name".into()).code(),
            "invalid_input:name"
        );
        assert_eq!(Error::Corrupt("payload".into()).code(), "corrupt:payload");
        assert_eq!(Error::Unsupported("hotp".into()).code(), "unsupported:hotp");
        let io = Error::Io(std::io::Error::other("disk full"));
        assert_eq!(io.code(), "io:disk full");
        let json = serde_json::from_str::<u32>("x").map_err(Error::from);
        assert!(json.err().is_some_and(|e| e.code().starts_with("corrupt:")));
    }

    #[test]
    fn io_at_names_path() {
        let e = Error::io_at(
            Path::new("/tmp/x.keystead"),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(e.code().starts_with("io:"));
        assert!(e.to_string().contains("x.keystead"));
    }
}
