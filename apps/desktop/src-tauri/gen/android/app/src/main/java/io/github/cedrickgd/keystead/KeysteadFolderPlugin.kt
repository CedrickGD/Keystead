// The phone's link to the user's own cloud folder (Block 5, stage 2a – see
// "Cloud-folder sync (phone)" in docs/ARCHITECTURE.md). The Nextcloud or
// OneDrive app offers its folders to the system picker (a
// DocumentsProvider); the user picks the folder the PC keeps its vault
// copies in, and the app keeps read access to it across restarts.
//
// Called from Rust only (src-tauri/src/mobile/sync.rs, plugin
// "keystead-folder"): no capability grants it to the webview. Read-only by
// design: the folder is listed and files are copied out of it, never
// written, and the copies go only to <app data>/sync/incoming. Provider
// calls can be slow (a cloud app may download a file first), so they run
// on a thread of their own, never on the UI thread. A unit test in
// src-tauri/src/gui.rs guards this file.
package io.github.cedrickgd.keystead

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
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
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
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

@TauriPlugin
class KeysteadFolderPlugin(private val activity: Activity) : Plugin(activity) {
  private val io = Executors.newSingleThreadExecutor()

  // Rejections carry a code the Rust side maps: cancelled, access_lost,
  // not_found, too_large, picker_unavailable, io.
  private class TooLarge : IOException("file too large")

  @Command
  fun pickFolder(invoke: Invoke) {
    val args = invoke.parseArgs(PickFolderArgs::class.java)
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
    args.initialUri?.let { intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, Uri.parse(it)) }
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
    try {
      // Read access only, kept across restarts (until releaseFolder or
      // until the user or the provider revokes it).
      activity.contentResolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
    } catch (e: SecurityException) {
      invoke.reject("the folder's permission cannot be kept", "access_lost")
      return
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
      })
    }
  }

  @Command
  fun releaseFolder(invoke: Invoke) {
    val args = invoke.parseArgs(TreeArgs::class.java)
    try {
      activity.contentResolver.releasePersistableUriPermission(
        Uri.parse(args.tree),
        Intent.FLAG_GRANT_READ_URI_PERMISSION
      )
    } catch (e: SecurityException) {
      // Not held (any more): nothing to give back.
    }
    invoke.resolve()
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

  // The tree, if the app still holds its persisted read permission.
  private fun granted(tree: String): Uri {
    val uri = Uri.parse(tree)
    val held = activity.contentResolver.persistedUriPermissions.any {
      it.uri == uri && it.isReadPermission
    }
    if (!held) throw SecurityException("no access to the folder")
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

  private fun displayName(tree: Uri): String? {
    val root = DocumentsContract.buildDocumentUriUsingTree(
      tree,
      DocumentsContract.getTreeDocumentId(tree)
    )
    activity.contentResolver.query(root, arrayOf(Document.COLUMN_DISPLAY_NAME), null, null, null)
      ?.use { c -> if (c.moveToFirst()) return c.getString(0) }
    return null
  }

  private fun reject(invoke: Invoke, e: Exception) {
    val code = when (e) {
      is TooLarge -> "too_large"
      // A revoked permission, or a provider that no longer knows the
      // folder (uninstalled, signed out).
      is SecurityException, is IllegalArgumentException -> "access_lost"
      is FileNotFoundException -> "not_found"
      else -> "io"
    }
    invoke.reject(e.message ?: e.javaClass.simpleName, code)
  }

  companion object {
    private val COLUMNS = arrayOf(
      Document.COLUMN_DOCUMENT_ID,
      Document.COLUMN_DISPLAY_NAME,
      Document.COLUMN_MIME_TYPE,
      Document.COLUMN_SIZE
    )
  }
}
