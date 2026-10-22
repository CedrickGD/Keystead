//! The phone's fingerprint unlock on top of [`crate::device_unlock`]: the
//! Android Keystore through the official `tauri-plugin-biometric` as the
//! [`Keystore`], and the commands `device_unlock_status`,
//! `enable_device_unlock`, `disable_device_unlock` and
//! `unlock_with_device`. See "Fingerprint unlock (phone)" in
//! docs/ARCHITECTURE.md.
//!
//! * The plugin's Rust API `biometric_cipher` (Android only) encrypts or
//!   decrypts a text with the AES-256-GCM key `tauri-plugin-biometric-key`
//!   in the Android Keystore (`setUserAuthenticationRequired(true)`, so a
//!   new fingerprint enrollment or a removed screen lock invalidates it),
//!   released only by a `BiometricPrompt` with a `CryptoObject` and
//!   `BIOMETRIC_STRONG` (Android 11+; on Android 10 the prompt's default
//!   for a crypto operation is strong biometrics as well). Keystead always
//!   passes `allowDeviceCredential = false`: no PIN or pattern instead of
//!   the finger. The pass/fail `authenticate` is never used – it would
//!   unlock nothing.
//! * Called from Rust only: no capability grants `biometric:*`, so the
//!   webview can neither prompt nor encrypt or decrypt anything itself.
//! * A plugin call blocks until the prompt's activity has answered, so it
//!   runs on the blocking pool (the commands), never on Android's UI
//!   thread. One prompt at a time.
//! * The prompt is an activity of its own: while it is open the main
//!   activity is paused, which the monitor does not take for the user
//!   leaving the app ([`Core::begin_own_prompt`]).

use std::sync::Mutex;
#[cfg(target_os = "android")]
use std::sync::TryLockError;

use keystead_core::model::VaultInfo;
use tauri::AppHandle;
use zeroize::Zeroizing;

use crate::commands::{run, CmdResult, Shared};
use crate::device_unlock::{
    self, DeviceUnlockStatus, Keystore, KeystoreError, Prompt, Unavailable,
};
use crate::state::Core;

/// Held while a prompt is open (one at a time).
static PROMPT: Mutex<()> = Mutex::new(());

/// The Android Keystore through the biometric plugin.
struct PluginKeystore<'a> {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    app: &'a AppHandle,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    core: &'a Core,
}

fn keystore<'a>(app: &'a AppHandle, core: &'a Core) -> PluginKeystore<'a> {
    PluginKeystore { app, core }
}

#[cfg(target_os = "android")]
impl PluginKeystore<'_> {
    /// One cipher operation behind the system prompt.
    fn cipher(
        &self,
        prompt: &Prompt,
        data_to_encrypt: Option<String>,
        data_to_decrypt: Option<String>,
    ) -> Result<String, KeystoreError> {
        use tauri_plugin_biometric::{AuthOptions, BiometricExt as _};

        let _one_at_a_time = match PROMPT.try_lock() {
            Ok(guard) => guard,
            // A panic while a prompt was open must not block the next one.
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(KeystoreError::Busy),
        };
        let opening = data_to_decrypt.is_some();
        let options = AuthOptions {
            allow_device_credential: false,
            title: Some(prompt.title.clone()),
            cancel_title: Some(prompt.cancel.clone()),
            data_to_encrypt,
            data_to_decrypt,
            ..AuthOptions::default()
        };
        let _prompt_open = self.core.begin_own_prompt();
        self.app
            .biometric()
            .biometric_cipher(prompt.reason.clone(), options)
            .map(|result| result.data)
            .map_err(|e| keystore_error(e, opening))
    }
}

/// The plugin's rejections as [`KeystoreError`]s
/// (`device_unlock::plugin_cipher_error`). `opening`: a decryption. The
/// messages are the plugin's (never a secret) and are logged.
#[cfg(target_os = "android")]
fn keystore_error(e: tauri_plugin_biometric::Error, opening: bool) -> KeystoreError {
    use tauri::plugin::mobile::PluginInvokeError;

    let tauri_plugin_biometric::Error::PluginInvoke(PluginInvokeError::InvokeRejected(rejected)) =
        e
    else {
        crate::state::log(format_args!("fingerprint unlock: {e}"));
        return KeystoreError::Other(e.to_string());
    };
    let message = rejected.message.unwrap_or_default();
    let error = device_unlock::plugin_cipher_error(rejected.code.as_deref(), &message, opening);
    if matches!(error, KeystoreError::Invalidated | KeystoreError::Other(_)) {
        crate::state::log(format_args!("fingerprint unlock: {message}"));
    }
    error
}

#[cfg(target_os = "android")]
impl Keystore for PluginKeystore<'_> {
    fn availability(&self) -> Result<(), Unavailable> {
        use tauri_plugin_biometric::BiometricExt as _;

        let status = self
            .app
            .biometric()
            .status()
            .map_err(|_| Unavailable::Other)?;
        device_unlock::plugin_availability(
            status.is_available,
            status.error_code.as_deref(),
            status.error.as_deref(),
        )
    }

    fn seal(&self, plaintext: &str, prompt: &Prompt) -> Result<String, KeystoreError> {
        // The copy goes to the plugin by value (JSON, JNI, Kotlin): see
        // "Fingerprint unlock (phone)" for what cannot be wiped there.
        self.cipher(prompt, Some(plaintext.to_owned()), None)
    }

    fn open(&self, sealed: &str, prompt: &Prompt) -> Result<Zeroizing<String>, KeystoreError> {
        self.cipher(prompt, None, Some(sealed.to_owned()))
            .map(Zeroizing::new)
    }
}

/// iOS (not built): the plugin has no keystore cipher there.
#[cfg(not(target_os = "android"))]
impl Keystore for PluginKeystore<'_> {
    fn availability(&self) -> Result<(), Unavailable> {
        Err(Unavailable::NoHardware)
    }

    fn seal(&self, _plaintext: &str, _prompt: &Prompt) -> Result<String, KeystoreError> {
        Err(KeystoreError::Unavailable(Unavailable::NoHardware))
    }

    fn open(&self, _sealed: &str, _prompt: &Prompt) -> Result<Zeroizing<String>, KeystoreError> {
        Err(KeystoreError::Unavailable(Unavailable::NoHardware))
    }
}

// ---------------------------------------------------------------------------
// Commands (phone only)
// ---------------------------------------------------------------------------

/// Whether the device can use the fingerprint, whether the feature is on
/// and which vaults open with it (tidies up setups that no longer fit, see
/// `device_unlock::status`). No prompt; not user activity.
#[tauri::command]
pub async fn device_unlock_status(
    app: AppHandle,
    core: Shared<'_>,
) -> CmdResult<DeviceUnlockStatus> {
    run(&core, false, move |c| {
        Ok(device_unlock::status(c, &keystore(&app, c)))
    })
    .await
}

/// Sets up fingerprint unlock for the open vault the page shows (the
/// system prompt confirms it) and turns `deviceUnlock` on.
#[tauri::command]
pub async fn enable_device_unlock(
    app: AppHandle,
    core: Shared<'_>,
    prompt: Prompt,
    page_vault_id: Option<String>,
) -> CmdResult<DeviceUnlockStatus> {
    run(&core, true, move |c| {
        device_unlock::enable(c, &keystore(&app, c), page_vault_id.as_deref(), &prompt)
    })
    .await
}

/// Removes the fingerprint unlock of `vault_id`, or of every vault
/// (`null`); the setting goes off with the last one.
#[tauri::command]
pub async fn disable_device_unlock(
    app: AppHandle,
    core: Shared<'_>,
    vault_id: Option<String>,
) -> CmdResult<DeviceUnlockStatus> {
    run(&core, true, move |c| {
        device_unlock::disable(c, &keystore(&app, c), vault_id.as_deref())
    })
    .await
}

/// Unlocks `vault_id` with the fingerprint (the system prompt).
#[tauri::command]
pub async fn unlock_with_device(
    app: AppHandle,
    core: Shared<'_>,
    vault_id: String,
    prompt: Prompt,
) -> CmdResult<VaultInfo> {
    run(&core, true, move |c| {
        device_unlock::unlock(c, &keystore(&app, c), &vault_id, &prompt)
    })
    .await
}
