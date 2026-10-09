//! Message types of the browser bridge protocol (see "Browser bridge
//! protocol" in docs/ARCHITECTURE.md).
//!
//! Request:  `{ "id": "<uuid>", "type": "<type>", "clientId"?: string, "token"?: string, ...payload }`
//! Response: `{ "id": "<same>", "ok": true, "data": ... }` or `{ "id", "ok": false, "error": "<code>" }`
//!
//! `Debug` output of every type that can carry a secret (passwords, tokens,
//! response data) is redacted, so requests and responses can be logged.

use std::fmt;
use std::time::Duration;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use vaultx_core::generator::GeneratorOptions;
use vaultx_core::totp::TotpCode;

/// How long a `pair` request waits for the user's decision in the app.
pub const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
/// Maximum number of results of a `search` request.
pub const SEARCH_LIMIT: usize = 50;

const REDACTED: &str = "<redacted>";

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

/// Stable protocol error codes (the `error` field of a failed response).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BridgeError {
    /// The request needs pairing and `clientId`/`token` are missing or invalid.
    NotPaired,
    /// The user denied the pairing request, or it timed out / was superseded.
    PairingDenied,
    /// The request needs an unlocked vault.
    Locked,
    /// Wrong master password (also while unlock attempts are rate-limited).
    WrongPassword,
    /// The item does not exist (or is not a login).
    NotFound,
    /// Malformed JSON, unknown `type`, missing or invalid fields.
    InvalidRequest,
    /// The native host could not reach the desktop app.
    AppUnavailable,
    /// Anything else (I/O error in the app, bug, ...).
    Internal,
}

impl BridgeError {
    /// Every error code, e.g. for exhaustive tests.
    pub const ALL: [BridgeError; 8] = [
        BridgeError::NotPaired,
        BridgeError::PairingDenied,
        BridgeError::Locked,
        BridgeError::WrongPassword,
        BridgeError::NotFound,
        BridgeError::InvalidRequest,
        BridgeError::AppUnavailable,
        BridgeError::Internal,
    ];

    /// The wire code, e.g. `"not_paired"`.
    pub const fn code(self) -> &'static str {
        match self {
            BridgeError::NotPaired => "not_paired",
            BridgeError::PairingDenied => "pairing_denied",
            BridgeError::Locked => "locked",
            BridgeError::WrongPassword => "wrong_password",
            BridgeError::NotFound => "not_found",
            BridgeError::InvalidRequest => "invalid_request",
            BridgeError::AppUnavailable => "app_unavailable",
            BridgeError::Internal => "internal",
        }
    }

    /// Parses a wire code.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.code() == code)
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for BridgeError {}

impl Serialize for BridgeError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for BridgeError {
    /// Unknown codes (from a newer app version) map to `Internal` instead of
    /// failing the whole response.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Ok(BridgeError::from_code(&code).unwrap_or(BridgeError::Internal))
    }
}

impl From<vaultx_core::Error> for BridgeError {
    /// Maps core errors for [`VaultBackend`](crate::dispatcher::VaultBackend)
    /// implementations: wrong password, not found and invalid input keep
    /// their meaning, everything else is `internal`.
    fn from(err: vaultx_core::Error) -> Self {
        match err {
            vaultx_core::Error::WrongPassword => BridgeError::WrongPassword,
            vaultx_core::Error::NotFound(_) => BridgeError::NotFound,
            vaultx_core::Error::InvalidInput(_) => BridgeError::InvalidRequest,
            _ => BridgeError::Internal,
        }
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// A request from the extension.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// `type` plus the type-specific fields, flattened into the request object.
    #[serde(flatten)]
    pub payload: Payload,
}

/// The type-specific part of a [`Request`], tagged by `"type"`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Payload {
    Status,
    Pair {
        client_name: String,
        code: String,
    },
    Unlock {
        password: String,
    },
    Lock,
    FocusApp,
    LoginsForUrl {
        url: String,
    },
    Search {
        query: String,
    },
    GetLogin {
        item_id: String,
    },
    GetTotp {
        item_id: String,
    },
    GeneratePassword {
        /// Partial options; missing fields take the generator defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<GeneratorOptions>,
    },
    SaveLogin {
        name: String,
        url: String,
        username: String,
        password: String,
    },
    UpdatePassword {
        item_id: String,
        password: String,
    },
}

impl Payload {
    /// The wire `type`, e.g. `"logins_for_url"`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Payload::Status => "status",
            Payload::Pair { .. } => "pair",
            Payload::Unlock { .. } => "unlock",
            Payload::Lock => "lock",
            Payload::FocusApp => "focus_app",
            Payload::LoginsForUrl { .. } => "logins_for_url",
            Payload::Search { .. } => "search",
            Payload::GetLogin { .. } => "get_login",
            Payload::GetTotp { .. } => "get_totp",
            Payload::GeneratePassword { .. } => "generate_password",
            Payload::SaveLogin { .. } => "save_login",
            Payload::UpdatePassword { .. } => "update_password",
        }
    }

    /// True if the request needs a valid `clientId` + `token`.
    pub fn needs_pairing(&self) -> bool {
        !matches!(
            self,
            Payload::Status | Payload::Pair { .. } | Payload::FocusApp
        )
    }

    /// True if the request needs an unlocked vault.
    pub fn needs_unlocked(&self) -> bool {
        matches!(
            self,
            Payload::LoginsForUrl { .. }
                | Payload::Search { .. }
                | Payload::GetLogin { .. }
                | Payload::GetTotp { .. }
                | Payload::SaveLogin { .. }
                | Payload::UpdatePassword { .. }
        )
    }

    /// True for requests that represent a deliberate user action in the
    /// browser (as opposed to background traffic such as `status` polling or
    /// the automatic `logins_for_url` on page load). Only these are reported
    /// to [`VaultBackend::on_activity`](crate::dispatcher::VaultBackend::on_activity),
    /// so the extension cannot keep the vault from auto-locking by itself.
    pub fn is_user_action(&self) -> bool {
        matches!(
            self,
            Payload::Unlock { .. }
                | Payload::Search { .. }
                | Payload::GetLogin { .. }
                | Payload::GetTotp { .. }
                | Payload::GeneratePassword { .. }
                | Payload::SaveLogin { .. }
                | Payload::UpdatePassword { .. }
        )
    }
}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Payload::Status | Payload::Lock | Payload::FocusApp => f.write_str(self.type_name()),
            Payload::Pair { client_name, code } => f
                .debug_struct("pair")
                .field("client_name", client_name)
                .field("code", code)
                .finish(),
            Payload::Unlock { .. } => f
                .debug_struct("unlock")
                .field("password", &REDACTED)
                .finish(),
            Payload::LoginsForUrl { url } => {
                f.debug_struct("logins_for_url").field("url", url).finish()
            }
            Payload::Search { query } => f.debug_struct("search").field("query", query).finish(),
            Payload::GetLogin { item_id } => f
                .debug_struct("get_login")
                .field("item_id", item_id)
                .finish(),
            Payload::GetTotp { item_id } => f
                .debug_struct("get_totp")
                .field("item_id", item_id)
                .finish(),
            Payload::GeneratePassword { options } => f
                .debug_struct("generate_password")
                .field("options", options)
                .finish(),
            Payload::SaveLogin {
                name,
                url,
                username,
                ..
            } => f
                .debug_struct("save_login")
                .field("name", name)
                .field("url", url)
                .field("username", username)
                .field("password", &REDACTED)
                .finish(),
            Payload::UpdatePassword { item_id, .. } => f
                .debug_struct("update_password")
                .field("item_id", item_id)
                .field("password", &REDACTED)
                .finish(),
        }
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("id", &self.id)
            .field("client_id", &self.client_id)
            .field("token", &self.token.as_ref().map(|_| REDACTED))
            .field("payload", &self.payload)
            .finish()
    }
}

impl Request {
    /// A request without credentials.
    pub fn new(id: impl Into<String>, payload: Payload) -> Self {
        Request {
            id: id.into(),
            client_id: None,
            token: None,
            payload,
        }
    }

    /// Adds pairing credentials.
    pub fn with_credentials(
        mut self,
        client_id: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        self.client_id = Some(client_id.into());
        self.token = Some(token.into());
        self
    }

    /// Parses a request frame. On failure returns the ready-to-send
    /// `invalid_request` response (with the request's `id` if it could be
    /// recovered, otherwise `""`). Never panics on any input.
    pub fn parse(bytes: &[u8]) -> Result<Request, Response> {
        serde_json::from_slice::<Request>(bytes).map_err(|_| {
            Response::error(
                extract_id(bytes).unwrap_or_default(),
                BridgeError::InvalidRequest,
            )
        })
    }
}

/// Best-effort extraction of the string `id` of a (possibly otherwise
/// invalid) message. `None` if the bytes are not a JSON object with a string
/// `id`.
pub fn extract_id(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct IdOnly {
        id: String,
    }
    serde_json::from_slice::<IdOnly>(bytes).ok().map(|m| m.id)
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// A response to a [`Request`]. Serialises as `{ id, ok: true, data }` or
/// `{ id, ok: false, error }` (the other field is omitted).
#[derive(Clone, PartialEq, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    /// Result data of a successful response (`null` for requests without a result).
    #[serde(default)]
    pub data: Value,
    /// Error code of a failed response.
    #[serde(default)]
    pub error: Option<BridgeError>,
}

impl Serialize for Response {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("ok", &self.ok)?;
        if self.ok {
            map.serialize_entry("data", &self.data)?;
        } else {
            map.serialize_entry("error", &self.error.unwrap_or(BridgeError::Internal))?;
        }
        map.end()
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Response");
        s.field("id", &self.id).field("ok", &self.ok);
        if self.ok {
            // Data may contain passwords, TOTP codes or a pairing token.
            s.field(
                "data",
                &if self.data.is_null() {
                    "null"
                } else {
                    REDACTED
                },
            );
        } else {
            s.field("error", &self.error);
        }
        s.finish()
    }
}

impl Response {
    /// A successful response. Falls back to an `internal` error if `data`
    /// cannot be represented as JSON (never happens for the protocol types).
    pub fn success<T: Serialize + ?Sized>(id: impl Into<String>, data: &T) -> Response {
        let id = id.into();
        match serde_json::to_value(data) {
            Ok(data) => Response {
                id,
                ok: true,
                data,
                error: None,
            },
            Err(_) => Response::error(id, BridgeError::Internal),
        }
    }

    /// A successful response with `"data": null`.
    pub fn null(id: impl Into<String>) -> Response {
        Response {
            id: id.into(),
            ok: true,
            data: Value::Null,
            error: None,
        }
    }

    /// A failed response.
    pub fn error(id: impl Into<String>, error: BridgeError) -> Response {
        Response {
            id: id.into(),
            ok: false,
            data: Value::Null,
            error: Some(error),
        }
    }

    /// `success` or `error` depending on `result`.
    pub fn from_result<T: Serialize>(
        id: impl Into<String>,
        result: Result<T, BridgeError>,
    ) -> Response {
        match result {
            Ok(data) => Response::success(id, &data),
            Err(e) => Response::error(id, e),
        }
    }

    /// The error code of a failed response (`None` if `ok`).
    pub fn error_code(&self) -> Option<BridgeError> {
        if self.ok {
            None
        } else {
            Some(self.error.unwrap_or(BridgeError::Internal))
        }
    }

    /// Deserialises `data` of a successful response into `T`.
    pub fn data_as<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        if self.ok {
            serde_json::from_value(self.data.clone()).ok()
        } else {
            None
        }
    }

    /// JSON bytes of this response.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_else(|_| {
            // Unreachable for a map of strings, bools and JSON values; keep a
            // well-formed fallback anyway instead of panicking.
            format!(
                r#"{{"id":{},"ok":false,"error":"internal"}}"#,
                Value::String(self.id.clone())
            )
            .into_bytes()
        })
    }
}

// ---------------------------------------------------------------------------
// Response data
// ---------------------------------------------------------------------------

/// `data` of `status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusData {
    pub app_version: String,
    pub paired: bool,
    pub unlocked: bool,
    /// Name of the unlocked vault; only revealed to paired clients.
    pub vault_name: Option<String>,
}

/// `data` of `pair`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairData {
    pub client_id: String,
    pub token: String,
}

impl fmt::Debug for PairData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairData")
            .field("client_id", &self.client_id)
            .field("token", &REDACTED)
            .finish()
    }
}

/// `data` of `unlock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlockData {
    pub vault_name: String,
}

/// `data` of `save_login` and `update_password`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdData {
    pub id: String,
}

/// `data` of `get_login`: everything needed to fill a login form.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginSecret {
    pub id: String,
    pub name: String,
    pub username: String,
    pub password: String,
    /// Current TOTP code if the login has a TOTP seed.
    pub totp: Option<TotpCode>,
    pub uris: Vec<String>,
}

impl fmt::Debug for LoginSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginSecret")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("username", &self.username)
            .field("password", &REDACTED)
            .field("totp", &self.totp.as_ref().map(|_| REDACTED))
            .field("uris", &self.uris)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_round_trip() {
        for e in BridgeError::ALL {
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(json, format!("\"{}\"", e.code()));
            assert_eq!(serde_json::from_str::<BridgeError>(&json).unwrap(), e);
            assert_eq!(e.to_string(), e.code());
        }
        assert_eq!(
            serde_json::from_str::<BridgeError>("\"from_the_future\"").unwrap(),
            BridgeError::Internal
        );
    }

    #[test]
    fn core_error_mapping() {
        use vaultx_core::Error as E;
        assert_eq!(
            BridgeError::from(E::WrongPassword),
            BridgeError::WrongPassword
        );
        assert_eq!(
            BridgeError::from(E::NotFound("x".into())),
            BridgeError::NotFound
        );
        assert_eq!(
            BridgeError::from(E::InvalidInput("x".into())),
            BridgeError::InvalidRequest
        );
        assert_eq!(BridgeError::from(E::Conflict), BridgeError::Internal);
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let req = Request::new(
            "1",
            Payload::SaveLogin {
                name: "n".into(),
                url: "u".into(),
                username: "me".into(),
                password: "hunter2".into(),
            },
        )
        .with_credentials("c", "tok-secret");
        let s = format!("{req:?}");
        assert!(!s.contains("hunter2") && !s.contains("tok-secret"), "{s}");
        let s = format!(
            "{:?}",
            Request::new(
                "1",
                Payload::Unlock {
                    password: "hunter2".into()
                }
            )
        );
        assert!(!s.contains("hunter2"), "{s}");

        let secret = LoginSecret {
            id: "i".into(),
            name: "n".into(),
            username: "u".into(),
            password: "hunter2".into(),
            totp: None,
            uris: vec![],
        };
        assert!(!format!("{secret:?}").contains("hunter2"));
        let resp = Response::success("1", &secret);
        assert!(!format!("{resp:?}").contains("hunter2"));
        let pair = PairData {
            client_id: "c".into(),
            token: "tok-secret".into(),
        };
        assert!(!format!("{pair:?}").contains("tok-secret"));
    }

    #[test]
    fn table_flags() {
        let p = Payload::Status;
        assert!(!p.needs_pairing() && !p.needs_unlocked());
        let p = Payload::Pair {
            client_name: "x".into(),
            code: "123456".into(),
        };
        assert!(!p.needs_pairing());
        assert!(!Payload::FocusApp.needs_pairing());
        let p = Payload::Unlock {
            password: String::new(),
        };
        assert!(p.needs_pairing() && !p.needs_unlocked());
        assert!(Payload::Lock.needs_pairing() && !Payload::Lock.needs_unlocked());
        let p = Payload::GeneratePassword { options: None };
        assert!(p.needs_pairing() && !p.needs_unlocked());
        let p = Payload::GetTotp {
            item_id: "x".into(),
        };
        assert!(p.needs_pairing() && p.needs_unlocked());
    }
}
