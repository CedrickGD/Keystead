//! Backend error type. Every Tauri command returns `Result<T, String>` where
//! the string is a stable error code (see "Desktop backend ↔ frontend" in
//! docs/ARCHITECTURE.md); [`AppError::code`] produces it.

use vaultx_bridge::BridgeError;

/// Error of a backend operation.
#[derive(Debug)]
pub enum AppError {
    /// No vault is unlocked.
    Locked,
    /// Error from `vaultx-core` (already has a stable code).
    Core(vaultx_core::Error),
    /// Error of the bridge infrastructure.
    Bridge(vaultx_bridge::Error),
    /// A ready-made code, e.g. `invalid_input:lock_first`.
    Code(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// `invalid_input:<detail>`.
    pub fn invalid(detail: impl std::fmt::Display) -> Self {
        AppError::Code(format!("invalid_input:{detail}"))
    }

    /// `unsupported:<detail>`.
    pub fn unsupported(detail: impl std::fmt::Display) -> Self {
        AppError::Code(format!("unsupported:{detail}"))
    }

    /// `io:<detail>`.
    pub fn io(detail: impl std::fmt::Display) -> Self {
        AppError::Code(format!("io:{detail}"))
    }

    /// The stable error code for the UI.
    pub fn code(&self) -> String {
        match self {
            AppError::Locked => "locked".to_owned(),
            AppError::Core(e) => e.code(),
            AppError::Bridge(e) => e.code(),
            AppError::Code(c) => c.clone(),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code())
    }
}

impl std::error::Error for AppError {}

impl From<vaultx_core::Error> for AppError {
    fn from(e: vaultx_core::Error) -> Self {
        AppError::Core(e)
    }
}

impl From<vaultx_bridge::Error> for AppError {
    fn from(e: vaultx_bridge::Error) -> Self {
        AppError::Bridge(e)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Core(vaultx_core::Error::Io(e))
    }
}

/// Lets commands use `?` on [`AppError`] while returning `Result<T, String>`.
impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.code()
    }
}

/// Maps backend errors to bridge protocol codes.
impl From<AppError> for BridgeError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Locked => BridgeError::Locked,
            AppError::Core(e) => BridgeError::from(e),
            AppError::Bridge(vaultx_bridge::Error::Core(e)) => BridgeError::from(e),
            AppError::Bridge(_) => BridgeError::Internal,
            AppError::Code(code) => {
                if code.starts_with("invalid_input") {
                    BridgeError::InvalidRequest
                } else if code == "not_found" {
                    BridgeError::NotFound
                } else {
                    BridgeError::Internal
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(AppError::Locked.code(), "locked");
        assert_eq!(
            AppError::from(vaultx_core::Error::WrongPassword).code(),
            "wrong_password"
        );
        assert_eq!(
            AppError::invalid("lock_first").code(),
            "invalid_input:lock_first"
        );
        let s: String = AppError::Locked.into();
        assert_eq!(s, "locked");
        assert_eq!(BridgeError::from(AppError::Locked), BridgeError::Locked);
        assert_eq!(
            BridgeError::from(AppError::invalid("x")),
            BridgeError::InvalidRequest
        );
        assert_eq!(
            BridgeError::from(AppError::from(vaultx_core::Error::NotFound("i".into()))),
            BridgeError::NotFound
        );
    }
}
