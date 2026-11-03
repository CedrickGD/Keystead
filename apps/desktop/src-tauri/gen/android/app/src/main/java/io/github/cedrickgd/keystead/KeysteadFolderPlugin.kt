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

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.storage.StorageManager
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.Settings
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

@InvokeArg
class BrowsePathArgs {
  var path: String? = null
}

@InvokeArg
class CreatePathFolderArgs {
  lateinit var parent: String
  lateinit var name: String
}

@InvokeArg
class CheckPathFolderArgs {
  lateinit var path: String
  var create: Boolean = false
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
    args.initialUri
      ?.takeIf { it.startsWith("content://") }
      ?.let { intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, Uri.parse(it)) }
    // Show advanced roots (SD cards, USB drives, NAS/SMB providers, internal storage).
    intent.putExtra(DocumentsContract.EXTRA_SHOW_ADVANCED, true)
    intent.putExtra("android.content.extra.SHOW_ADVANCED", true)
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
    if (tree.scheme == "file") {
      io.execute {
        try {
          val dir = pathDir(tree.path ?: "")
          if (!dir.isDirectory || !dir.canRead()) {
            throw FileNotFoundException("the folder cannot be listed")
          }
          invoke.resolve(JSObject().apply {
            put("tree", Uri.fromFile(dir).toString())
            put("name", dir.path)
            put("writable", dir.canWrite())
          })
        } catch (e: Exception) {
          reject(invoke, e)
        }
      }
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
        if (activity.checkCallingOrSelfUriPermission(
            tree,
            Intent.FLAG_GRANT_READ_URI_PERMISSION
          ) != PackageManager.PERMISSION_GRANTED
        ) {
          invoke.reject("the folder's permission cannot be kept", "access_lost")
          return
        }
        activity.checkCallingOrSelfUriPermission(
          tree,
          Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        ) == PackageManager.PERMISSION_GRANTED
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
    if (uri.scheme == "file") {
      invoke.resolve()
      return
    }
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
    if (uri.scheme == "file") {
      val dir = try {
        fileRoot(args.tree, needWrite = false)
      } catch (e: Exception) {
        null
      }
      invoke.resolve(JSObject().apply {
        put("read", dir != null && dir.canRead())
        put("write", dir != null && dir.canWrite())
      })
      return
    }
    val held = activity.contentResolver.persistedUriPermissions.firstOrNull { it.uri == uri }
    val canRead = held?.isReadPermission == true ||
      activity.checkCallingOrSelfUriPermission(
        uri,
        Intent.FLAG_GRANT_READ_URI_PERMISSION
      ) == PackageManager.PERMISSION_GRANTED
    val canWrite = held?.isWritePermission == true ||
      activity.checkCallingOrSelfUriPermission(
        uri,
        Intent.FLAG_GRANT_WRITE_URI_PERMISSION
      ) == PackageManager.PERMISSION_GRANTED
    invoke.resolve(JSObject().apply {
      put("read", canRead)
      put("write", canWrite)
    })
  }

  @Command
  fun storageAccess(invoke: Invoke) {
    io.execute {
      invoke.resolve(storageAccessObject())
    }
  }

  @Command
  fun requestStorageAccess(invoke: Invoke) {
    if (hasAllFilesAccess()) {
      io.execute { invoke.resolve(storageAccessObject()) }
      return
    }
    try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        val intent = Intent(
          Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
          Uri.parse("package:${activity.packageName}")
        )
        try {
          startActivityForResult(invoke, intent, "storageAccessResult")
        } catch (e: ActivityNotFoundException) {
          val fallback = Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)
          startActivityForResult(invoke, fallback, "storageAccessResult")
        }
      } else {
        val intent = Intent(
          Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
          Uri.fromParts("package", activity.packageName, null)
        )
        startActivityForResult(invoke, intent, "storageAccessResult")
      }
    } catch (e: Exception) {
      io.execute { invoke.resolve(storageAccessObject()) }
    }
  }

  @ActivityCallback
  private fun storageAccessResult(invoke: Invoke, result: ActivityResult) {
    io.execute {
      invoke.resolve(storageAccessObject())
    }
  }

  @Command
  fun browsePath(invoke: Invoke) {
    val args = invoke.parseArgs(BrowsePathArgs::class.java)
    io.execute {
      try {
        val raw = args.path?.trim()?.takeIf { it.isNotEmpty() } ?: defaultStoragePath()
        val target = File(raw)
        val dir = try {
          if (target.exists()) target.canonicalFile else target.absoluteFile
        } catch (e: IOException) {
          target.absoluteFile
        }
        forbidAppData(dir)
        val exists = dir.exists() && dir.isDirectory
        val readable = exists && dir.canRead()
        val writable = exists && dir.canWrite()
        val entries = JSArray()
        val listed = LinkedHashMap<String, File>()
        if (exists) {
          dir.listFiles()?.forEach { child ->
            if (child.isDirectory && !isInsideAppData(child)) {
              listed[child.name] = child
            }
          }
          if (dir.path == "/storage" && listed.isEmpty()) {
            for (vol in mountedVolumes()) {
              if (vol.parentFile?.path == "/storage" && !isInsideAppData(vol)) {
                listed[vol.name] = vol
              }
            }
            val emulated = File("/storage/emulated/0")
            if (emulated.isDirectory) {
              listed["emulated/0"] = emulated
            }
          }
        }
        listed.entries
          .sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.key })
          .forEach { (label, child) ->
            entries.put(JSObject().apply {
              put("name", label)
              put("path", child.path)
            })
          }
        val parentPath = dir.parentFile
          ?.takeIf { it.path != dir.path && !isInsideAppData(it) }
          ?.path
        invoke.resolve(JSObject().apply {
          put("path", dir.path)
          put("parent", parentPath)
          put("exists", exists)
          put("readable", readable)
          put("writable", writable)
          put("granted", hasAllFilesAccess())
          put("roots", storageRootsArray())
          put("entries", entries)
        })
      } catch (e: Exception) {
        reject(invoke, e)
      }
    }
  }

  @Command
  fun createPathFolder(invoke: Invoke) {
    val args = invoke.parseArgs(CreatePathFolderArgs::class.java)
    io.execute {
      try {
        val trimmed = args.name.trim()
        if (trimmed.isEmpty() || trimmed == "." || trimmed == ".." ||
          trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\u0000')
        ) {
          throw Invalid("invalid folder name")
        }
        val parentDir = pathDir(args.parent)
        if (!parentDir.exists() || !parentDir.isDirectory) {
          throw FileNotFoundException("the parent folder does not exist")
        }
        val created = File(parentDir, trimmed).canonicalFile
        forbidAppData(created)
        if (created.parentFile != parentDir) {
          throw Invalid("invalid folder name")
        }
        if (created.exists()) {
          if (!created.isDirectory) throw NameTaken()
        } else if (!created.mkdirs()) {
          if (!parentDir.canWrite()) throw NotWritable()
          throw IOException("the folder cannot be created")
        }
        invoke.resolve(JSObject().apply {
          put("path", created.path)
        })
      } catch (e: Exception) {
        reject(invoke, e)
      }
    }
  }

  @Command
  fun checkPathFolder(invoke: Invoke) {
    val args = invoke.parseArgs(CheckPathFolderArgs::class.java)
    io.execute {
      try {
        val dir = pathDir(args.path)
        if (!dir.exists()) {
          if (!args.create) throw FileNotFoundException("the folder does not exist")
          if (!dir.mkdirs() && !dir.isDirectory) {
            throw IOException("the folder cannot be created")
          }
        }
        if (!dir.isDirectory) {
          throw FileNotFoundException("the path is not a folder")
        }
        if (!dir.canRead()) {
          throw SecurityException("no access to the folder")
        }
        invoke.resolve(JSObject().apply {
          put("tree", Uri.fromFile(dir).toString())
          put("name", dir.path)
          put("writable", dir.canWrite())
        })
      } catch (e: Exception) {
        reject(invoke, e)
      }
    }
  }

  // The network the app would use now: { kind } – "wifi" (Wi-Fi or
  // Ethernet, or another network that is not metered – a VPN over Wi-Fi
  // shows only its own transport), "cellular", "other" or "none".
  @Command
  fun networkKind(invoke: Invoke) {
    val manager = activity.getSystemService(ConnectivityManager::class.java)
    val caps = manager?.activeNetwork?.let { manager.getNetworkCapabilities(it) }
    val kind = when {
      caps == null -> "none"
      caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
        caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "wifi"
      caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
      caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) -> "wifi"
      else -> "other"
    }
    invoke.resolve(JSObject().apply { put("kind", kind) })
  }

  @Command
  fun listChildren(invoke: Invoke) {
    val args = invoke.parseArgs(ListChildrenArgs::class.java)
    io.execute {
      try {
        val fileDir = fileRoot(args.tree)
        if (fileDir != null) {
          val dir = args.parent?.let { fileInRoot(fileDir, it) } ?: fileDir
          if (!dir.isDirectory || !dir.canRead()) {
            throw FileNotFoundException("the folder cannot be listed")
          }
          val children = dir.listFiles()
            ?: throw FileNotFoundException("the folder cannot be listed")
          val entries = JSArray()
          for (child in children) {
            val isDir = child.isDirectory
            entries.put(JSObject().apply {
              put("name", child.name)
              put("uri", Uri.fromFile(child).toString())
              put("dir", isDir)
              if (!isDir) put("size", child.length())
            })
          }
          invoke.resolve(JSObject().apply { put("entries", entries) })
          return@execute
        }
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
        val fileDir = fileRoot(args.tree)
        if (fileDir != null) {
          val sourceFile = fileInRoot(fileDir, args.uri)
          if (!sourceFile.isFile || !sourceFile.canRead()) {
            throw FileNotFoundException("the file cannot be opened")
          }
          val target = incomingFile(args.target)
          if (!target.createNewFile()) throw IOException("the target exists")
          created = target
          var total = 0L
          FileInputStream(sourceFile).use { from ->
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
          return@execute
        }
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
        val fileDir = fileRoot(args.tree, needWrite = true)
        if (fileDir != null) {
          if (!VAULT_FILE_NAME.matches(args.name)) throw Invalid("not a vault file name")
          val source = outgoingFile(args.source, args.name)
          val target = File(fileDir, args.name)
          val written = copyIntoFile(target, source, overwrite = true)
          invoke.resolve(JSObject().apply {
            put("uri", Uri.fromFile(target).toString())
            put("size", written)
          })
          return@execute
        }
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
        val fileDir = fileRoot(args.tree, needWrite = true)
        if (fileDir != null) {
          if (!CONFLICT_COPY_NAME.matches(args.name)) throw Invalid("not a conflict copy name")
          val source = outgoingFile(args.source, args.name)
          val target = File(fileDir, args.name)
          if (target.exists()) throw NameTaken()
          val written = copyIntoFile(target, source, overwrite = false)
          invoke.resolve(JSObject().apply {
            put("uri", Uri.fromFile(target).toString())
            put("size", written)
          })
          return@execute
        }
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

  private fun copyIntoFile(target: File, source: File, overwrite: Boolean): Long {
    val parent = target.parentFile?.canonicalFile ?: throw FileNotFoundException("no parent folder")
    forbidAppData(parent)
    if (target.canonicalFile.parentFile != parent) {
      throw SecurityException("the file is not in the linked folder")
    }
    if (!overwrite) {
      if (!target.createNewFile()) throw NameTaken()
    } else if (target.isDirectory) {
      throw NameTaken()
    }
    var written = 0L
    FileOutputStream(target, false).use { to ->
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
        // Network/FUSE mounts may not support fsync; Rust read-back verifies bytes.
      }
    }
    val size = target.length()
    if (written != source.length() || size != written) {
      throw WriteUnverified()
    }
    return written
  }

  private fun hasAllFilesAccess(): Boolean {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      Environment.isExternalStorageManager()
    } else {
      activity.checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE) ==
        PackageManager.PERMISSION_GRANTED &&
        activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) ==
        PackageManager.PERMISSION_GRANTED
    }
  }

  private fun defaultStoragePath(): String {
    val primary = try {
      Environment.getExternalStorageDirectory()?.path
    } catch (e: Exception) {
      null
    }
    return primary?.takeIf { it.isNotEmpty() } ?: "/storage/emulated/0"
  }

  private fun mountedVolumes(): List<File> {
    val found = LinkedHashMap<String, File>()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      val sm = activity.getSystemService(StorageManager::class.java)
      sm?.storageVolumes?.forEach { vol ->
        vol.directory?.let { dir ->
          if (!isInsideAppData(dir)) found[dir.path] = dir
        }
      }
    }
    activity.getExternalFilesDirs(null)?.forEach { appDir ->
      val root = appDir?.parentFile?.parentFile?.parentFile?.parentFile
      if (root != null && root.isDirectory && !isInsideAppData(root)) {
        found[root.path] = root
      }
    }
    File("/storage").listFiles()?.forEach { child ->
      if (child.isDirectory && child.name != "emulated" && child.name != "self" && !isInsideAppData(child)) {
        found[child.path] = child
      }
    }
    return found.values.toList()
  }

  private fun storageRootsArray(): JSArray {
    val roots = JSArray()
    val seen = HashSet<String>()
    fun addRoot(name: String, path: String) {
      if (seen.add(path)) {
        roots.put(JSObject().apply {
          put("name", name)
          put("path", path)
        })
      }
    }
    val primary = defaultStoragePath()
    addRoot(primary, primary)
    for (vol in mountedVolumes()) {
      if (vol.path != primary) {
        addRoot(vol.name.ifEmpty { vol.path }, vol.path)
      }
    }
    addRoot("/storage", "/storage")
    addRoot("/mnt", "/mnt")
    addRoot("/", "/")
    return roots
  }

  private fun storageAccessObject(): JSObject {
    return JSObject().apply {
      put("granted", hasAllFilesAccess())
      put("roots", storageRootsArray())
    }
  }

  private fun isInsideAppData(file: File): Boolean {
    return try {
      val data = activity.dataDir.canonicalFile.toPath()
      val candidate = file.canonicalFile.toPath()
      candidate.startsWith(data)
    } catch (e: IOException) {
      true
    }
  }

  private fun forbidAppData(file: File) {
    if (isInsideAppData(file)) {
      throw Invalid("the app's private data folder is not allowed")
    }
  }

  private fun pathDir(raw: String): File {
    val trimmed = raw.trim()
    if (trimmed.isEmpty() || !trimmed.startsWith("/") || trimmed.contains('\u0000')) {
      throw Invalid("not an absolute folder path")
    }
    val dir = File(trimmed).canonicalFile
    forbidAppData(dir)
    return dir
  }

  private fun fileRoot(tree: String, needWrite: Boolean = false): File? {
    val uri = Uri.parse(tree)
    if (uri.scheme != "file") return null
    val rawPath = uri.path ?: throw SecurityException("no access to the folder")
    val dir = pathDir(rawPath)
    if (!dir.exists() || !dir.isDirectory || !dir.canRead()) {
      throw SecurityException("no access to the folder")
    }
    if (needWrite && !dir.canWrite()) throw NotWritable()
    return dir
  }

  private fun fileInRoot(root: File, childUri: String): File {
    val uri = Uri.parse(childUri)
    if (uri.scheme != "file") {
      throw SecurityException("the document is not in the linked folder")
    }
    val rawPath = uri.path ?: throw SecurityException("the document is not in the linked folder")
    val canonicalRoot = root.canonicalFile
    val canonicalChild = File(rawPath).canonicalFile
    forbidAppData(canonicalChild)
    if (!canonicalChild.toPath().startsWith(canonicalRoot.toPath()) || canonicalChild == canonicalRoot) {
      throw SecurityException("the document is not in the linked folder")
    }
    return canonicalChild
  }

  // The tree, if the app still holds its persisted read (and, with
  // `needWrite`, write) permission.
  private fun granted(tree: String, needWrite: Boolean = false): Uri {
    val uri = Uri.parse(tree)
    val held = activity.contentResolver.persistedUriPermissions.firstOrNull {
      it.uri == uri && it.isReadPermission
    }
    if (held != null) {
      if (needWrite && !held.isWritePermission) throw NotWritable()
      return uri
    }
    val transientRead = activity.checkCallingOrSelfUriPermission(
      uri,
      Intent.FLAG_GRANT_READ_URI_PERMISSION
    ) == PackageManager.PERMISSION_GRANTED
    if (!transientRead) throw SecurityException("no access to the folder")
    if (needWrite) {
      val transientWrite = activity.checkCallingOrSelfUriPermission(
        uri,
        Intent.FLAG_GRANT_WRITE_URI_PERMISSION
      ) == PackageManager.PERMISSION_GRANTED
      if (!transientWrite) throw NotWritable()
    }
    return uri
  }

  // The id of a document URI that lies inside `tree` (from an earlier
  // listing); anything else is refused.
  private fun documentIdIn(tree: Uri, document: String): String {
    val uri = Uri.parse(document)
    val isDoc = DocumentsContract.isDocumentUri(activity, uri) ||
      runCatching { DocumentsContract.getDocumentId(uri) }.isSuccess
    if (uri.authority != tree.authority ||
      !isDoc ||
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
