package app.homespace.android

import android.content.Context
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import android.util.Log
import android.webkit.MimeTypeMap
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.json.JSONException
import org.json.JSONObject
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.DownloadListener
import uniffi.homespace_core.Entry
import uniffi.homespace_core.EntryKind
import uniffi.homespace_core.QueuedUploadState

private const val TAG = "HomeSpaceDocuments"

/** The root's document: Home. */
private const val HOME_ID = "home"

/** A file saved here that is still in the upload queue, by its queue key. */
private const val PENDING_PREFIX = "upload:"

/** Under the cache folder: downloads on their way to a caller. */
private const val READ_DIR = "documents"

/** What files saved here were called, so their ids still resolve once they are on the server. */
private const val PENDING_PREFS = "provider_uploads"
private const val PENDING_DAYS = 7L
private const val PENDING_MILLIS = PENDING_DAYS * 24 * 60 * 60 * 1000

/** How often to look again while the app itself is downloading the same file. */
private const val BUSY_RETRY_MILLIS = 300L

private const val DEFAULT_MIME = "application/octet-stream"

private val ROOT_COLUMNS = arrayOf(
    Root.COLUMN_ROOT_ID,
    Root.COLUMN_DOCUMENT_ID,
    Root.COLUMN_TITLE,
    Root.COLUMN_SUMMARY,
    Root.COLUMN_ICON,
    Root.COLUMN_FLAGS,
)

private val DOCUMENT_COLUMNS = arrayOf(
    Document.COLUMN_DOCUMENT_ID,
    Document.COLUMN_MIME_TYPE,
    Document.COLUMN_DISPLAY_NAME,
    Document.COLUMN_LAST_MODIFIED,
    Document.COLUMN_SIZE,
    Document.COLUMN_FLAGS,
)

/**
 * Home Space in the system file picker: pick a file, for example to attach it to an email, or save
 * one into a folder, for example an email attachment. See `docs/todo/mobile-file-provider.md`.
 *
 * It runs in the app's process and uses the one client of [ClientHolder]. Reads download to a
 * private file and hand over a descriptor on it; they never keep an offline copy. Saved files go
 * through the same upload queue as files picked in the app, so they carry on when the caller is
 * gone and show in Transfers. A name that exists already gets a new version.
 *
 * The system calls in on binder threads, never the main one, so blocking here is allowed.
 */
class HomeSpaceDocuments : DocumentsProvider() {
    private lateinit var closeHandler: Handler

    override fun onCreate(): Boolean {
        val thread = HandlerThread("documents-close").apply { start() }
        closeHandler = Handler(thread.looper)
        // A read is deleted once its descriptor is open, so anything here is from a run that died.
        val leftovers = File(context!!.cacheDir, READ_DIR)
        Thread { leftovers.deleteRecursively() }.start()
        return true
    }

    private val appContext: Context
        get() = context!!.applicationContext

    // Roots

    override fun queryRoots(projection: Array<out String>?): Cursor {
        val cursor = MatrixCursor(projection ?: ROOT_COLUMNS)
        cursor.setNotificationUri(appContext.contentResolver, rootsUri(appContext))
        val saved = SessionStore(appContext).loadSession() ?: return cursor
        cursor.newRow()
            .add(Root.COLUMN_ROOT_ID, HOME_ID)
            .add(Root.COLUMN_DOCUMENT_ID, HOME_ID)
            .add(Root.COLUMN_TITLE, appContext.getString(R.string.app_name))
            .add(Root.COLUMN_SUMMARY, Uri.parse(saved.server).host ?: saved.server)
            .add(Root.COLUMN_ICON, R.mipmap.ic_launcher)
            .add(Root.COLUMN_FLAGS, Root.FLAG_SUPPORTS_CREATE)
        return cursor
    }

    // Documents

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor {
        val cursor = MatrixCursor(projection ?: DOCUMENT_COLUMNS)
        when {
            documentId == HOME_ID -> addHome(cursor)
            documentId.startsWith(PENDING_PREFIX) -> addPending(cursor, documentId)
            else -> addEntry(cursor, documentId, entry(documentId))
        }
        return cursor
    }

    override fun queryChildDocuments(
        parentDocumentId: String,
        projection: Array<out String>?,
        sortOrder: String?,
    ): Cursor {
        val parent = parentDocumentId.takeUnless { it == HOME_ID }
        val entries = call { session().client.list(parent) }
        val cursor = MatrixCursor(projection ?: DOCUMENT_COLUMNS)
        cursor.setNotificationUri(appContext.contentResolver, childrenUri(parentDocumentId))
        for (entry in entries) {
            known[entry.id] = entry
            addEntry(cursor, entry.id, entry)
        }
        return cursor
    }

    override fun getDocumentType(documentId: String): String = when {
        documentId == HOME_ID -> Document.MIME_TYPE_DIR
        documentId.startsWith(PENDING_PREFIX) ->
            pendingRecord(documentId.removePrefix(PENDING_PREFIX))?.mime ?: DEFAULT_MIME
        else -> entry(documentId).documentMime()
    }

    override fun createDocument(parentDocumentId: String, mimeType: String, displayName: String): String {
        val parent = parentDocumentId.takeUnless { it == HOME_ID }
        if (parent != null && parent.startsWith(PENDING_PREFIX)) {
            throw FileNotFoundException("Not a folder")
        }
        val session = session()
        val id = if (mimeType == Document.MIME_TYPE_DIR) {
            val folder = call { session.client.createFolder(parent, displayName) }
            known[folder.id] = folder
            folder.id
        } else {
            val mime = mimeType.takeUnless { it.isBlank() || it == DEFAULT_MIME } ?: guessMime(displayName)
            val key = call { session.uploads.add(parent, folderName(parent), displayName, null, mime) }
            rememberPending(key, PendingRecord(parent, displayName, mime, System.currentTimeMillis()))
            PENDING_PREFIX + key
        }
        appContext.contentResolver.notifyChange(childrenUri(parentDocumentId), null)
        return id
    }

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?,
    ): ParcelFileDescriptor {
        if (documentId.startsWith(PENDING_PREFIX)) {
            return openForSaving(documentId.removePrefix(PENDING_PREFIX), mode)
        }
        if (documentId == HOME_ID || mode != "r") {
            throw FileNotFoundException("$documentId can only be read")
        }
        return openForReading(documentId, signal)
    }

    // Reading

    /** Downloads the whole file first: the server can't serve a part of one. */
    private fun openForReading(id: String, signal: CancellationSignal?): ParcelFileDescriptor {
        val client = session().client
        val dir = File(appContext.cacheDir, READ_DIR).apply { mkdirs() }
        val file = File(dir, UUID.randomUUID().toString())
        signal?.setOnCancelListener { runCatching { client.cancelDownload(id) } }
        try {
            runBlocking {
                while (true) {
                    signal?.throwIfCanceled()
                    try {
                        client.saveCopy(id, file.path, NoProgress)
                        break
                    } catch (e: CoreException.Failed) {
                        // The app is downloading it too; only one download of a file runs at once.
                        if (e.code != "already_downloading") throw e
                        delay(BUSY_RETRY_MILLIS)
                    }
                }
            }
            // The descriptor keeps the content readable, so nothing is left to clean up.
            return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
        } catch (e: CoreException) {
            throw notFound(e)
        } finally {
            file.delete()
        }
    }

    // Saving

    /** The caller writes to the queue's copy; it goes up once the caller closes it. */
    private fun openForSaving(key: String, mode: String): ParcelFileDescriptor {
        if ('w' !in mode) throw FileNotFoundException("This file is still being saved")
        val uploads = session().uploads
        val item = call { uploads.items() }.find { it.key == key }
        if (item == null || item.state != QueuedUploadState.PREPARING) {
            throw FileNotFoundException("This file has already been saved")
        }
        val file = File(uploads.copyPath(key))
        file.parentFile?.mkdirs()
        val flags = ParcelFileDescriptor.MODE_WRITE_ONLY or
            ParcelFileDescriptor.MODE_CREATE or
            ParcelFileDescriptor.MODE_TRUNCATE
        return ParcelFileDescriptor.open(file, flags, closeHandler) { error ->
            finishSaving(key, error)
        }
    }

    private fun finishSaving(key: String, error: IOException?) {
        val uploads = ClientHolder.active?.uploads ?: return
        try {
            if (error == null) {
                uploads.ready(key, null)
                uploads.start()
                // Carries the upload on if this process goes once the caller is done.
                scheduleUploads(appContext)
            } else {
                Log.w(TAG, "the caller failed while saving", error)
                uploads.cancel(key)
            }
        } catch (e: CoreException) {
            Log.w(TAG, "could not queue the saved file", e)
        } catch (e: IllegalStateException) {
            // WorkManager refused; the upload already runs in this process.
            Log.w(TAG, "could not schedule the upload", e)
        }
        pendingRecord(key)?.let { appContext.contentResolver.notifyChange(childrenUri(it.parent ?: HOME_ID), null) }
    }

    // Rows

    private fun addHome(cursor: MatrixCursor) {
        cursor.newRow()
            .add(Document.COLUMN_DOCUMENT_ID, HOME_ID)
            .add(Document.COLUMN_MIME_TYPE, Document.MIME_TYPE_DIR)
            .add(Document.COLUMN_DISPLAY_NAME, "Home")
            .add(Document.COLUMN_FLAGS, Document.FLAG_DIR_SUPPORTS_CREATE)
    }

    private fun addEntry(cursor: MatrixCursor, documentId: String, entry: Entry) {
        val folder = entry.kind == EntryKind.FOLDER
        cursor.newRow()
            .add(Document.COLUMN_DOCUMENT_ID, documentId)
            .add(Document.COLUMN_MIME_TYPE, entry.documentMime())
            .add(Document.COLUMN_DISPLAY_NAME, entry.name)
            .add(Document.COLUMN_LAST_MODIFIED, entry.updatedAt.toEpochMilli())
            .add(Document.COLUMN_SIZE, if (folder) null else entry.size)
            .add(Document.COLUMN_FLAGS, if (folder) Document.FLAG_DIR_SUPPORTS_CREATE else 0)
    }

    /** A saved file: from the queue while it is there, then the file it became on the server. */
    private fun addPending(cursor: MatrixCursor, documentId: String) {
        val key = documentId.removePrefix(PENDING_PREFIX)
        val record = pendingRecord(key) ?: throw FileNotFoundException(documentId)
        val queued = ClientHolder.active?.uploads?.let { uploads ->
            runCatching { uploads.items() }.getOrNull()?.find { it.key == key }
        }
        if (queued == null) {
            val uploaded = call { session().client.list(record.parent) }
                .find { it.kind == EntryKind.FILE && it.name == record.name }
            if (uploaded != null) {
                addEntry(cursor, documentId, uploaded)
                return
            }
        }
        cursor.newRow()
            .add(Document.COLUMN_DOCUMENT_ID, documentId)
            .add(Document.COLUMN_MIME_TYPE, record.mime)
            .add(Document.COLUMN_DISPLAY_NAME, queued?.name ?: record.name)
            .add(Document.COLUMN_LAST_MODIFIED, record.createdAt)
            .add(Document.COLUMN_SIZE, queued?.total?.toLong()?.takeIf { it > 0 })
            .add(Document.COLUMN_FLAGS, Document.FLAG_SUPPORTS_WRITE.takeIf {
                queued?.state == QueuedUploadState.PREPARING
            } ?: 0)
    }

    // Lookups

    private fun entry(id: String): Entry =
        known[id] ?: call { session().client.details(id) }.entry.also { known[id] = it }

    private fun folderName(parent: String?): String =
        if (parent == null) "Home" else entry(parent).name

    private fun session(): ActiveSession = try {
        runBlocking { ClientHolder.resume(appContext) }
    } catch (e: CoreException) {
        throw notFound(e)
    } ?: throw FileNotFoundException("Not signed in to Home Space")

    private fun <T> call(block: suspend () -> T): T = try {
        runBlocking { block() }
    } catch (e: CoreException) {
        throw notFound(e)
    }

    private fun notFound(e: CoreException): FileNotFoundException =
        FileNotFoundException(e.fileMessage()).apply { initCause(e) }

    private fun rememberPending(key: String, record: PendingRecord) {
        val prefs = appContext.getSharedPreferences(PENDING_PREFS, Context.MODE_PRIVATE)
        val now = System.currentTimeMillis()
        val edit = prefs.edit()
        for ((old, value) in prefs.all) {
            val created = (value as? String)?.let(PendingRecord::parse)?.createdAt ?: 0
            if (now - created > PENDING_MILLIS) edit.remove(old)
        }
        edit.putString(key, record.encode()).commit()
    }

    private fun pendingRecord(key: String): PendingRecord? =
        appContext.getSharedPreferences(PENDING_PREFS, Context.MODE_PRIVATE)
            .getString(key, null)
            ?.let(PendingRecord::parse)

    private fun childrenUri(parentDocumentId: String): Uri =
        DocumentsContract.buildChildDocumentsUri(authority(appContext), parentDocumentId)

    companion object {
        /** What `queryChildDocuments` saw, so `queryDocument` needn't ask the server. */
        private val known = ConcurrentHashMap<String, Entry>()

        fun authority(context: Context): String = "${context.packageName}.documents"

        fun rootsUri(context: Context): Uri = DocumentsContract.buildRootsUri(authority(context))

        /** Someone signed in or out: the picker shows the root, or stops showing it. */
        fun sessionChanged(context: Context, signedIn: Boolean) {
            val appContext = context.applicationContext
            if (!signedIn) {
                known.clear()
                appContext.getSharedPreferences(PENDING_PREFS, Context.MODE_PRIVATE).edit().clear().commit()
            }
            appContext.contentResolver.notifyChange(rootsUri(appContext), null)
        }
    }
}

private object NoProgress : DownloadListener {
    override fun progress(id: String, done: Long, total: Long) {}
}

private fun Entry.documentMime(): String = when (kind) {
    EntryKind.FOLDER -> Document.MIME_TYPE_DIR
    EntryKind.FILE -> mime.ifBlank { DEFAULT_MIME }
}

private fun guessMime(name: String): String =
    MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
        ?: DEFAULT_MIME

/** A file saved here, as created: where it went, its name and type, and when. */
private data class PendingRecord(val parent: String?, val name: String, val mime: String, val createdAt: Long) {
    fun encode(): String = JSONObject()
        .put("parent", parent ?: JSONObject.NULL)
        .put("name", name)
        .put("mime", mime)
        .put("created", createdAt)
        .toString()

    companion object {
        fun parse(text: String): PendingRecord? = try {
            val json = JSONObject(text)
            PendingRecord(
                parent = if (json.isNull("parent")) null else json.getString("parent"),
                name = json.getString("name"),
                mime = json.getString("mime"),
                createdAt = json.getLong("created"),
            )
        } catch (_: JSONException) {
            null
        }
    }
}
