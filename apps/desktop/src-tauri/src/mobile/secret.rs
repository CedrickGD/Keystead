//! The sync server's password on the phone: the Kotlin plugin
//! `KeysteadSecretPlugin` (gen/android) seals it with an AES-256-GCM key of
//! the Android Keystore – the [`Sealer`] of `crate::credentials` there.
//!
//! * Called from Rust only: no capability grants `keystead-secret:*` to the
//!   webview (a unit test in `gui.rs` checks the capability files), so the
//!   page can neither seal nor open anything.
//! * The sealed value is the plugin's Base64 text (IV + ciphertext), stored
//!   as it is; the server key's hash is its associated data.
//! * A plugin call blocks until Android's UI thread has run it (the
//!   Keystore work runs on the plugin's own thread): only ever called from
//!   the sync's threads and the commands' blocking pool. The password
//!   crosses the plugin bridge as a JSON string (Kotlin strings cannot be
//!   wiped); it is never logged.

use tauri::plugin::TauriPlugin;
#[cfg(target_os = "android")]
use tauri::Manager;
use tauri::{AppHandle, Wry};

use crate::state::Core;

/// The plugin's name (`plugin:keystead-secret|…` – never granted to the
/// webview).
pub const PLUGIN_NAME: &str = "keystead-secret";

/// The Tauri plugin that loads the Kotlin side (Android) – no commands of
/// its own on the Rust side.
pub fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new(PLUGIN_NAME)
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            app.manage(android::SecretPlugin(api.register_android_plugin(
                "io.github.cedrickgd.keystead",
                "KeysteadSecretPlugin",
            )?));
            // iOS: no Keystore plugin yet (the password stays in memory).
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}

/// `gui::setup`, after the core exists: the credentials are sealed by the
/// plugin from now on.
pub fn install(app: &AppHandle, core: &Core) {
    #[cfg(target_os = "android")]
    if let Some(plugin) = app.try_state::<android::SecretPlugin>() {
        core.credentials()
            .set_sealer(std::sync::Arc::new(android::SecretPlugin(plugin.0.clone())));
    }
    #[cfg(not(target_os = "android"))]
    let _ = (app, core);
}

#[cfg(target_os = "android")]
mod android {
    use serde::{Deserialize, Serialize};
    use tauri::plugin::mobile::PluginInvokeError;
    use tauri::plugin::PluginHandle;
    use tauri::Wry;
    use zeroize::Zeroizing;

    use crate::credentials::Sealer;
    use crate::error::{AppError, AppResult};

    /// `KeysteadSecretPlugin` (gen/android/app/src/main/java/…), registered
    /// by [`super::plugin`].
    pub struct SecretPlugin(pub PluginHandle<Wry>);

    #[derive(Serialize)]
    struct SealArgs<'a> {
        context: &'a str,
        plain: &'a str,
    }

    #[derive(Deserialize)]
    struct Sealed {
        sealed: String,
    }

    #[derive(Serialize)]
    struct OpenArgs<'a> {
        context: &'a str,
        sealed: &'a str,
    }

    #[derive(Deserialize)]
    struct Opened {
        plain: String,
    }

    /// The plugin's rejections as codes (never its message: it could name
    /// a Keystore detail, never a secret – still not needed).
    fn plugin_error(e: PluginInvokeError) -> AppError {
        match e {
            PluginInvokeError::InvokeRejected(r) if r.code.as_deref() == Some("invalid") => {
                AppError::invalid("credential")
            }
            _ => AppError::io("keystore unavailable"),
        }
    }

    impl Sealer for SecretPlugin {
        fn seal(&self, context: &str, plain: &[u8]) -> AppResult<Vec<u8>> {
            let plain = std::str::from_utf8(plain).map_err(|_| AppError::invalid("credential"))?;
            let sealed = self
                .0
                .run_mobile_plugin::<Sealed>("seal", SealArgs { context, plain })
                .map_err(plugin_error)?;
            Ok(sealed.sealed.into_bytes())
        }

        fn open(&self, context: &str, sealed: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
            let sealed =
                std::str::from_utf8(sealed).map_err(|_| AppError::invalid("credential"))?;
            let opened = self
                .0
                .run_mobile_plugin::<Opened>("open", OpenArgs { context, sealed })
                .map_err(plugin_error)?;
            Ok(Zeroizing::new(opened.plain.into_bytes()))
        }
    }
}
