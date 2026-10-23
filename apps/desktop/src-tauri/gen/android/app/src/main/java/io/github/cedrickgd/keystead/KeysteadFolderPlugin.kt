// The phone's link to the user's own cloud folder (Block 5, stages 2a and
// 2b – see "Cloud-folder sync (phone)" in docs/ARCHITECTURE.md). The
// Nextcloud or OneDrive app offers its folders to the system picker (a
// DocumentsProvider); the user picks the folder the PC keeps its vault
// copies in, and the app keeps read and – where the provider grants it –
// write access to it across restarts.
//
// Called from Rust only (src-tauri/src/mobile/sync.rs, plugin
// "keystead-folder"): no capability grants it to the webview. The folder is
// listed and files are copied out of it – the copies go only to
// <app data>/sync/incoming. Writes are as narrow: only a top-level
// <id>.keystead file, only from <app data>/sync/outgoing, truncated and
// rewritten in place or created under exactly that name – and the phone's
// conflict copies (<id>.keystead.conflict-<YYYYMMDD-HHMMSS>-phone), only
// ever created, never written over; never a blob, and no file of the
// folder is ever deleted, renamed or moved. Provider
// calls can be slow (a cloud app may download or upload a file first), so
// they run on a thread of their own, never on the UI thread.
//
// Besides the folder, the sync asks one thing of the system here:
// `networkKind` – whether the phone is on Wi-Fi or mobile data right now
// ("Nur über WLAN synchronisieren" for a server link; ACCESS_NETWORK_STATE,
// no network I/O). A unit test in src-tauri/src/gui.rs guards this file.
package io.github.cedrickgd.keystead

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.io.SyncFailedException
import java.util.concurrent.Executors

@InvokeArg
class PickFolderArgs {
  // The linked folder, so the picker opens there when the user changes it.
  var initialUri: String? = null
}

@InvokeArg
class TreeArgs {
  lateinit var tree: String
}

@InvokeArg
class ListChildrenArgs {
  lateinit var tree: String
  // A folder inside the tree (a document URI from an earlier listing);
  // null = the tree's top level.
  var parent: String? = null
}

@InvokeArg
class CopyFileArgs {
  lateinit var tree: String
  lateinit var uri: String
  lateinit var target: String
  var maxBytes: Long = 0
}

@InvokeArg
class WriteFileArgs {
  lateinit var tree: String
  // The file's name at the tree's top level: <vault id>.keystead.
  lateinit var name: String
  // <app data>/sync/outgoing/<name>, staged by the Rust side.
  lateinit var source: String
}

@InvokeArg
class WriteConflictCopyArgs {
  lateinit var tree: String
  // <vault id>.keystead.conflict-<YYYYMMDD-HHMMSS>-phone.
  lateinit var name: String
  // <app data>/sync/outgoing/<name>, staged by the Rust side.
  lateinit var source: String
}

@TauriPlugin
class KeysteadFolderPlugin(private val activity: Activity) : Plugin(activity) {
  private val io = Executors.newSingleThreadExecutor()

  // Rejections carry a code the Rust side maps: cancelled, access_lost,
  // not_found, too_large, picker_unavailable, not_writable, name_taken,
  // write_unverified, invalid, io.
  private class TooLarge : IOException("file too large")
  private class NotWritable : IOException("no write access to the folder")
  private class NameTaken : IOException("the provider would not use that name")
  private class WriteUnverified : IOException("the written size differs")
  private class Invalid(message: String) : IOException(message)

  @Command
  fun pickFolder(invoke: Invoke) {
    val args = invoke.parseArgs(PickFolderArgs::class.java)
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
    args.initialUri?.let { intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, Uri.parse(it)) }
    // Ask for write access too: the phone writes its edits back.
    intent.addFlags(READ_WRITE or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
    try {
      startActivityForResult(invoke, intent, "folderPicked")
    } catch (e: ActivityNotFoundException) {
      invoke.reject("no folder picker on this device", "picker_unavailable")
    }
  }

  @ActivityCallback
  private fun folderPicked(invoke: Invoke, result: ActivityResult) {
    val tree = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || tree == null) {
      invoke.reject("no folder chosen", "cancelled")
      return
    }
    val resolver = activity.contentResolver
    // Read and write access, kept across restarts (until releaseFolder or
    // until the user or the provider revokes it). A provider that grants
    // read access only: the link stays read-only (the sync reports the
    // phone's edits as not written).
    val writable = try {
      resolver.takePersistableUriPermission(tree, READ_WRITE)
      true
    } catch (e: SecurityException) {
      try {
        resolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        false
      } catch (e: SecurityException) {
        invoke.reject("the folder's permission cannot be kept", "access_lost")
        return
      }
    }
    io.execute {
      val name = try {
        displayName(tree)
      } catch (e: Exception) {
        null
      }
      invoke.resolve(JSObject().apply {
        put("tree", tree.toString())
        put("name", name ?: tree.lastPathSegment ?: "")
        put("writable", writable)
      })
    }
  }

  @Command
  fun releaseFolder(invoke: Invoke) {
    val args = invoke.parseArgs(TreeArgs::class.java)
    val uri = Uri.parse(args.tree)
    val held = activity.contentResolver.persistedUriPermissions.firstOrNull { it.uri == uri }
    if (held != null) {
      var flags = 0
      if (held.isReadPermission) flags = flags or Intent.FLAG_GRANT_READ_URI_PERMISSION
      if (held.isWritePermission) flags = flags or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
      try {
        activity.contentResolver.releasePersistableUriPermission(uri, flags)
      } catch (e: SecurityException) {
        // Not held (any more): nothing to give back.
      }
    }
    invoke.resolve()
  }

  // Which persisted permissions the app holds for the tree: { read, write }.
  @Command
  fun folderAccess(invoke: Invoke) {
    val args = invoke.parseArgs(TreeArgs::class.java)
    val uri = Uri.parse(args.tree)
    val held = activity.contentResolver.persistedUriPermissions.firstOrNull { it.uri == uri }
    invoke.resolve(JSObject().apply {
      put("read", held?.isReadPermission == true)
      put("write", held?.isWritePermission == true)
    })
  }

  // The network the app would use now: { kind } – "wifi" (Wi-Fi or
  // Ethernet), "cellular", "other" or "none".
  @Command
  fun networkKind(invoke: Invoke) {
    val manager = activity.getSystemService(ConnectivityManager::class.java)
    val caps = manager?.activeNetwork?.let { manager.getNetworkCapabilities(it) }
    val kind = when {
      caps == null -> "none"
      caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
        caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "wifi"
      caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
      else -> "other"
    }
    invoke.resolve(JSObject().apply { put("kind", kind) })
  }

  @Command
  fun listChildren(invoke: Invoke) {
    val args = invoke.parseArgs(ListChildrenArgs::class.java)
    io.execute {
      try {
        val tree = granted(args.tree)
        val parentId = args.parent?.let { documentIdIn(tree, it) }
          ?: DocumentsContract.getTreeDocumentId(tree)
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId)
        val entries = JSArray()
        val cursor = activity.contentResolver.query(children, COLUMNS, null, null, null)
          ?: throw FileNotFoundException("the folder cannot be listed")
        cursor.use { c ->
          while (c.moveToNext()) {
            val id = c.getString(0) ?: continue
            val name = c.getString(1) ?: continue
            entries.put(JSObject().apply {
              put("name", name)
              put("uri", DocumentsContract.buildDocumentUriUsingTree(tree, id).toString())
              put("dir", c.getString(2) == Document.MIME_TYPE_DIR)
              if (!c.isNull(3)) put("size", c.getLong(3))
            })
          }
        }
        invoke.resolve(JSObject().apply { put("entries", entries) })
      } catch (e: Exception) {
        reject(invoke, e)
      }
    }
  }

  @Command
  fun copyFile(invoke: Invoke) {
    val args = invoke.parseArgs(CopyFileArgs::class.java)
    io.execute {
      var created: File? = null
      try {
        val tree = granted(args.tree)
        documentIdIn(tree, args.uri)
        val target = incomingFile(args.target)
        if (!target.createNewFile()) throw IOException("the target exists")
        created = target
        var total = 0L
        val input = activity.contentResolver.openInputStream(Uri.parse(args.uri))
          ?: throw FileNotFoundException("the file cannot be opened")
        input.use { from ->
          FileOutputStream(target).use { to ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
              val n = from.read(buffer)
              if (n < 0) break
              total += n
              if (total > args.maxBytes) throw TooLarge()
              to.write(buffer, 0, n)
            }
            to.fd.sync()
          }
        }
        created = null
        invoke.resolve(JSObject().apply { put("size", total) })
      } catch (e: Exception) {
        created?.delete()
        reject(invoke, e)
      }
    }
  }

  // Writes the staged file `source` as the top-level file `name` of the
  // tree: the file of that name is truncated and rewritten ("wt"), or –
  // when the folder has none – created under exactly that name. Returns
  // { uri, size }. The Rust side reads the file back and compares bytes.
  @Command
  fun writeFile(invoke: Invoke) {
    val args = invoke.parseArgs(WriteFileArgs::class.java)
    io.execute {
      try {
        val tree = granted(args.tree, needWrite = true)
        if (!VAULT_FILE_NAME.matches(args.name)) throw Invalid("not a vault file name")
        val source = outgoingFile(args.source, args.name)
        val resolver = activity.contentResolver
        val rootId = DocumentsContract.getTreeDocumentId(tree)
        val target = childNamed(tree, rootId, args.name) ?: run {
          val root = DocumentsContract.buildDocumentUriUsingTree(tree, rootId)
          val made = DocumentsContract.createDocument(
            resolver, root, "application/octet-stream", args.name
          ) ?: throw IOException("the file cannot be created")
          // A provider that already has a file of that name it did not
          // list picks another one (`… (1).keystead`): that new, empty
          // file is left alone (the sync reports it as misnamed) and
          // nothing is written into it.
          if (nameOf(made) != args.name) throw NameTaken()
          made
        }
        val written = copyInto(target, source, "wt")
        invoke.resolve(JSObject().apply {
          put("uri", target.toString())
          put("size", written)
        })
      } catch (e: Exception) {
        reject(invoke, e)
      }
    }
  }

  // Creates the top-level file `name` – a conflict copy of the phone's
  // (`<id>.keystead.conflict-<YYYYMMDD-HHMMSS>-phone`) – from the staged
  // file `source`. Never written over: when the folder lists a file of
  // that name, or the provider picks another name, it is `name_taken` (a
  // new, empty file the provider made under another name is left alone).
  // Returns { uri, size }.
  @Command
  fun writeConflictCopy(invoke: Invoke) {
    val args = invoke.parseArgs(WriteConflictCopyArgs::class.java)
    io.execute {
      try {
        val tree = granted(args.tree, needWrite = true)
        if (!CONFLICT_COPY_NAME.matches(args.name)) throw Invalid("not a conflict copy name")
        val source = outgoingFile(args.source, args.name)
        val rootId = DocumentsContract.getTreeDocumentId(tree)
        if (childNamed(tree, rootId, args.name) != null) throw NameTaken()
        val root = DocumentsContract.buildDocumentUriUsingTree(tree, rootId)
        val made = DocumentsContract.createDocument(
          activity.contentResolver, root, "application/octet-stream", args.name
        ) ?: throw IOException("the file cannot be created")
        if (nameOf(made) != args.name) throw NameTaken()
        val written = copyInto(made, source, "w")
        invoke.resolve(JSObject().apply {
          put("uri", made.toString())
          put("size", written)
        })
      } catch (e: Exception) {
        reject(invoke, e)
      }
    }
  }

  // Writes the staged file `source` into the document `target` (opened
  // with `mode`), synced to disk where the provider allows it; the size is
  // checked. Returns the bytes written.
  private fun copyInto(target: Uri, source: File, mode: String): Long {
    var written = 0L
    val descriptor = activity.contentResolver.openFileDescriptor(target, mode)
      ?: throw FileNotFoundException("the file cannot be opened for writing")
    descriptor.use { pfd ->
      FileOutputStream(pfd.fileDescriptor).use { to ->
        FileInputStream(source).use { from ->
          val buffer = ByteArray(64 * 1024)
          while (true) {
            val n = from.read(buffer)
            if (n < 0) break
            to.write(buffer, 0, n)
            written += n
          }
        }
        to.flush()
        try {
          to.fd.sync()
        } catch (e: SyncFailedException) {
          // A pipe to the provider cannot be synced; the read-back on the
          // Rust side verifies the file instead.
        }
      }
    }
    // Not every provider reports a size (then the read-back decides).
    val size = sizeOf(target)
    if (written != source.length() || (size != null && size != written)) {
      throw WriteUnverified()
    }
    return written
  }

  // The tree, if the app still holds its persisted read (and, with
  // `needWrite`, write) permission.
  private fun granted(tree: String, needWrite: Boolean = false): Uri {
    val uri = Uri.parse(tree)
    val held = activity.contentResolver.persistedUriPermissions.firstOrNull {
      it.uri == uri && it.isReadPermission
    } ?: throw SecurityException("no access to the folder")
    if (needWrite && !held.isWritePermission) throw NotWritable()
    return uri
  }

  // The id of a document URI that lies inside `tree` (from an earlier
  // listing); anything else is refused.
  private fun documentIdIn(tree: Uri, document: String): String {
    val uri = Uri.parse(document)
    if (uri.authority != tree.authority ||
      !DocumentsContract.isDocumentUri(activity, uri) ||
      DocumentsContract.getTreeDocumentId(uri) != DocumentsContract.getTreeDocumentId(tree)
    ) {
      throw SecurityException("the document is not in the linked folder")
    }
    return DocumentsContract.getDocumentId(uri)
  }

  // Copies go to <app data>/sync/incoming only (the Rust side reads, checks
  // and removes them there).
  private fun incomingFile(path: String): File {
    val file = File(path).canonicalFile
    val incoming = file.parentFile
    val data = activity.dataDir.canonicalFile
    if (incoming == null || incoming.name != "incoming" ||
      incoming.parentFile?.name != "sync" || incoming.parentFile?.parentFile != data
    ) {
      throw SecurityException("the target is not the app's incoming folder")
    }
    return file
  }

  // Writes come from <app data>/sync/outgoing/<name> only (the Rust side
  // stages the vault file there and removes it afterwards).
  private fun outgoingFile(path: String, name: String): File {
    val file = File(path).canonicalFile
    val outgoing = file.parentFile
    val data = activity.dataDir.canonicalFile
    if (outgoing == null || outgoing.name != "outgoing" ||
      outgoing.parentFile?.name != "sync" || outgoing.parentFile?.parentFile != data ||
      file.name != name || !file.isFile
    ) {
      throw Invalid("the source is not the app's outgoing file")
    }
    return file
  }

  // The top-level file named exactly `name` (not a folder), if the
  // provider lists one.
  private fun childNamed(tree: Uri, parentId: String, name: String): Uri? {
    val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId)
    val cursor = activity.contentResolver.query(children, COLUMNS, null, null, null)
      ?: throw FileNotFoundException("the folder cannot be listed")
    cursor.use { c ->
      while (c.moveToNext()) {
        val id = c.getString(0) ?: continue
        if (c.getString(1) == name && c.getString(2) != Document.MIME_TYPE_DIR) {
          return DocumentsContract.buildDocumentUriUsingTree(tree, id)
        }
      }
    }
    return null
  }

  private fun nameOf(document: Uri): String? {
    activity.contentResolver.query(document, arrayOf(Document.COLUMN_DISPLAY_NAME), null, null, null)
      ?.use { c -> if (c.moveToFirst()) return c.getString(0) }
    return null
  }

  private fun sizeOf(document: Uri): Long? {
    activity.contentResolver.query(document, arrayOf(Document.COLUMN_SIZE), null, null, null)
      ?.use { c -> if (c.moveToFirst() && !c.isNull(0)) return c.getLong(0) }
    return null
  }

  private fun displayName(tree: Uri): String? {
    val root = DocumentsContract.buildDocumentUriUsingTree(
      tree,
      DocumentsContract.getTreeDocumentId(tree)
    )
    return nameOf(root)
  }

  private fun reject(invoke: Invoke, e: Exception) {
    val code = when (e) {
      is TooLarge -> "too_large"
      is NotWritable -> "not_writable"
      is NameTaken -> "name_taken"
      is WriteUnverified -> "write_unverified"
      is Invalid -> "invalid"
      // A revoked permission, or a provider that no longer knows the
      // folder (uninstalled, signed out).
      is SecurityException, is IllegalArgumentException -> "access_lost"
      is FileNotFoundException -> "not_found"
      else -> "io"
    }
    invoke.reject(e.message ?: e.javaClass.simpleName, code)
  }

  companion object {
    private const val READ_WRITE =
      Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION

    // A vault file as the app names it (`sync::vault_id_of_file_name`).
    private val VAULT_FILE_NAME = Regex("^[A-Za-z0-9_-]{1,64}\\.keystead$")

    // A conflict copy of the phone's (`sync::phone_conflict_file_name`).
    private val CONFLICT_COPY_NAME =
      Regex("^[A-Za-z0-9_-]{1,64}\\.keystead\\.conflict-[0-9]{8}-[0-9]{6}-phone$")

    private val COLUMNS = arrayOf(
      Document.COLUMN_DOCUMENT_ID,
      Document.COLUMN_DISPLAY_NAME,
      Document.COLUMN_MIME_TYPE,
      Document.COLUMN_SIZE
    )
  }
}
