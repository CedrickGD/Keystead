// Seals the sync server's password (stage 2c) with a key of the Android
// Keystore, so `<app data>/sync/credentials/<hash>.bin` holds it only
// encrypted (see "Server credentials" in docs/ARCHITECTURE.md).
//
// Called from Rust only (src-tauri/src/mobile/secret.rs, plugin
// "keystead-secret"): no capability grants it to the webview, so the page
// can neither seal nor open anything. The key – AES-256-GCM, alias
// keystead-sync-credential – is made inside the Keystore and never leaves
// it: no user authentication per use (the sync runs in the background of
// the app), but only usable while the phone is unlocked. Not the
// fingerprint unlock's key, no shared preferences. The Rust side
// binds every sealed value to its file (associated data `context`, the
// server key's hash). Nothing here is logged. A unit test in
// src-tauri/src/gui.rs guards this file.
package io.github.cedrickgd.keystead

import android.app.Activity
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.KeyStore
import java.util.concurrent.Executors
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@InvokeArg
class SealArgs {
  // The server key's hash (associated data).
  lateinit var context: String
  // The password (UTF-8).
  lateinit var plain: String
}

@InvokeArg
class OpenArgs {
  lateinit var context: String
  // Base64 of IV (12 bytes) + ciphertext with tag, as `seal` returned it.
  lateinit var sealed: String
}

@TauriPlugin
class KeysteadSecretPlugin(private val activity: Activity) : Plugin(activity) {
  private val worker = Executors.newSingleThreadExecutor()

  // Rejections carry a code the Rust side maps: invalid, unavailable.
  @Command
  fun seal(invoke: Invoke) {
    val args = invoke.parseArgs(SealArgs::class.java)
    worker.execute {
      try {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(args.context.toByteArray(Charsets.UTF_8))
        val plain = args.plain.toByteArray(Charsets.UTF_8)
        val sealed = try {
          cipher.iv + cipher.doFinal(plain)
        } finally {
          plain.fill(0)
        }
        invoke.resolve(JSObject().apply { put("sealed", Base64.encodeToString(sealed, Base64.NO_WRAP)) })
      } catch (e: Exception) {
        invoke.reject("the password cannot be sealed", "unavailable")
      }
    }
  }

  @Command
  fun open(invoke: Invoke) {
    val args = invoke.parseArgs(OpenArgs::class.java)
    worker.execute {
      val bytes = try {
        Base64.decode(args.sealed, Base64.NO_WRAP)
      } catch (e: IllegalArgumentException) {
        null
      }
      if (bytes == null || bytes.size <= IV_BYTES + TAG_BYTES) {
        invoke.reject("not a sealed value", "invalid")
        return@execute
      }
      try {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(
          Cipher.DECRYPT_MODE,
          existingKey() ?: throw IllegalStateException("no key"),
          GCMParameterSpec(TAG_BYTES * 8, bytes, 0, IV_BYTES)
        )
        cipher.updateAAD(args.context.toByteArray(Charsets.UTF_8))
        val plain = cipher.doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES)
        try {
          invoke.resolve(JSObject().apply { put("plain", String(plain, Charsets.UTF_8)) })
        } finally {
          plain.fill(0)
        }
      } catch (e: Exception) {
        // Another key (the app's data restored elsewhere, the Keystore
        // reset), a changed file, or the phone is locked right now.
        invoke.reject("the password cannot be opened", "unavailable")
      }
    }
  }

  private fun existingKey(): SecretKey? {
    val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
    return store.getKey(ALIAS, null) as? SecretKey
  }

  // The key, made on first use.
  private fun key(): SecretKey {
    existingKey()?.let { return it }
    val spec = KeyGenParameterSpec.Builder(
      ALIAS,
      KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
    )
      .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
      .setKeySize(256)
      .setRandomizedEncryptionRequired(true)
      .setUserAuthenticationRequired(false)
      .setUnlockedDeviceRequired(true)
      .build()
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
    generator.init(spec)
    return generator.generateKey()
  }

  companion object {
    private const val KEYSTORE = "AndroidKeyStore"
    private const val ALIAS = "keystead-sync-credential"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val IV_BYTES = 12
    private const val TAG_BYTES = 16
  }
}
