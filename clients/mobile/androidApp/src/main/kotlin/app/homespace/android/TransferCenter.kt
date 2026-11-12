package app.homespace.android

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import android.widget.Toast
import androidx.compose.runtime.mutableStateMapOf
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.DownloadListener
import uniffi.homespace_core.SendFile
import uniffi.homespace_core.SendTarget
import uniffi.homespace_core.TransferRow
import uniffi.homespace_core.TransferStatus
import uniffi.homespace_core.encodeUriList
import uniffi.homespace_core.uriListFromText

private const val SEND_DIR = "send"
/** Text shared from another app, written out so it goes like a file. */
private const val SHARED_TEXT_DIR = "shared-text"
private const val TEXT_PLAIN = "text/plain"
/** A list of links, one per line (RFC 2483): the receiving device offers to open it. */
private const val URI_LIST = "text/uri-list"
private const val URI_LIST_EXTENSION = "uris"
private const val RECEIVE_DIR = "transfers"
/** The folder under Download that received files go to. */
private const val DOWNLOADS_FOLDER = "Home Space"
private const val OCTET_STREAM = "application/octet-stream"
private const val INCOMING_CHANNEL = "transfers"
private const val INCOMING_CHANNEL_NAME = "Transfers"
private const val INCOMING_CHANNEL_DESCRIPTION = "Files sent to this phone from your other devices or other users"
/** The notification id of a received transfer; its tag is the transfer's id. */
private const val INCOMING_NOTIFICATION = 0
/** The request code of the intent a received transfer's notification opens the app with. */
private const val OPEN_TRANSFERS_REQUEST = 1
private const val SENDING_CHANNEL = "sending"
private const val SENDING_NOTIFICATION = 4
private const val SEND_WORK = "send-to-device"
private const val PREFS = "transfers"
private const val NOTIFIED = "notified"
private const val ASKED_NOTIFICATIONS = "asked_notifications"

/** Set on the intent that opens the app from a transfer's notification. */
const val EXTRA_SHOW_TRANSFERS = "app.homespace.android.SHOW_TRANSFERS"

/** The id of a received transfer to open, from the Open action of its notification. */
const val EXTRA_OPEN_TRANSFER = "app.homespace.android.OPEN_TRANSFER"

/**
 * Files handed to the app to save or send, as far as their provider describes them. `mimes` holds
 * each one's type, null where it isn't known. `text` is set when what was shared is text, such as
 * a link, written out to the one file.
 */
data class SharedFiles(
    val uris: List<Uri>,
    val names: List<String>,
    val bytes: Long,
    val mimes: List<String?> = uris.map { null },
    val text: String? = null,
    /** The links of `text` when it is nothing but links, which then go as a list of links. */
    val links: List<String>? = null,
)

/** Copies of files to send, in a folder of their own that goes once they are sent. */
class PreparedSend(val files: List<SendFile>, val dir: File)

/**
 * The sends and downloads of transfers under way. They belong to the process, not to a screen:
 * the share screen closes as soon as a send has started.
 */
object TransferCenter {
    private val scope = MainScope()
    private val listChanges = MutableStateFlow(0)
    private val sending = MutableStateFlow(0)

    /** How far each running send or download is, from 0 to 1, by the transfer's id. */
    val progress = mutableStateMapOf<String, Float>()

    /** Changes whenever the lists of transfers may have changed because of something done here. */
    val changes: StateFlow<Int> = listChanges

    /** How many sends from this phone are uploading. */
    val sends: StateFlow<Int> = sending

    fun changed() {
        listChanges.update { it + 1 }
    }

    private val deviceChanges = MutableStateFlow(0)

    /** Changes whenever a device may have come online, gone offline or been signed out. */
    val devices: StateFlow<Int> = deviceChanges

    fun devicesChanged() {
        deviceChanges.update { it + 1 }
    }

    /** Follows one send or download; `id` is the transfer's once it is known. */
    private class Tracker(var id: String?) : DownloadListener {
        private var ended = false

        override fun progress(id: String, done: Long, total: Long) {
            val fraction = if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else 1f
            scope.launch {
                // A report can arrive after the call has ended.
                if (ended) return@launch
                progress[id] = fraction
                if (this@Tracker.id == null) {
                    this@Tracker.id = id
                    // The new transfer is on the server now, so the Sent list can show it.
                    changed()
                }
            }
        }

        fun end() {
            ended = true
            id?.let(progress::remove)
        }
    }

    /** Uploads `prepared` to `target`. It carries on whichever screen is open, or none. */
    fun send(context: Context, client: CoreClient, prepared: PreparedSend, target: SendTarget) {
        val appContext = context.applicationContext
        sending.update { it + 1 }
        keepSending(appContext)
        scope.launch {
            val tracker = Tracker(null)
            try {
                client.sendFiles(prepared.files, target.userId, target.deviceId, tracker)
                toast(appContext, "Sent to ${target.recipient}")
            } catch (_: CoreException.Cancelled) {
            } catch (e: CoreException) {
                toast(appContext, "Could not send to ${target.recipient}: ${e.fileMessage()}")
            } finally {
                tracker.end()
                withContext(Dispatchers.IO) { prepared.dir.deleteRecursively() }
                sending.update { it - 1 }
                changed()
            }
        }
    }

    /** Downloads a received transfer to the phone's Download folder, then tells the server. */
    fun download(context: Context, client: CoreClient, row: TransferRow, onSessionEnded: () -> Unit) {
        val appContext = context.applicationContext
        if (row.id in progress) return
        progress[row.id] = 0f
        val tracker = Tracker(row.id)
        scope.launch {
            val dir = File(appContext.cacheDir, "$RECEIVE_DIR/${row.id}")
            try {
                val files = client.downloadTransfer(row.id, dir.path, tracker)
                val where = withContext(Dispatchers.IO) { exportToDownloads(appContext, dir, files) }
                client.confirmTransfer(row.id)
                toast(appContext, "Saved “${row.title}” to $where")
            } catch (_: CoreException.Cancelled) {
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                toast(appContext, "Could not download “${row.title}”: ${e.fileMessage()}")
            } catch (e: IOException) {
                toast(appContext, "Could not save “${row.title}”: ${e.message}")
            } finally {
                tracker.end()
                withContext(Dispatchers.IO) { dir.deleteRecursively() }
                changed()
            }
        }
    }

    /** Removes what an earlier run of the app left half done. */
    fun sweep(context: Context) {
        if (sending.value > 0 || progress.isNotEmpty()) return
        val cache = context.applicationContext.cacheDir
        Thread {
            File(cache, SEND_DIR).deleteRecursively()
            File(cache, RECEIVE_DIR).deleteRecursively()
            File(cache, SHARED_TEXT_DIR).deleteRecursively()
        }.start()
    }

    private fun toast(context: Context, message: String) {
        Toast.makeText(context, message, Toast.LENGTH_LONG).show()
    }
}

/** The names and sizes of `uris`, as far as their providers say. */
fun describeShared(context: Context, uris: List<Uri>): SharedFiles {
    val resolver = context.contentResolver
    var bytes = 0L
    val names = uris.mapIndexed { index, uri ->
        var name: String? = null
        try {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        name = cursor.getString(0)
                        if (!cursor.isNull(1)) bytes += cursor.getLong(1)
                    }
                }
        } catch (_: RuntimeException) {
            // A provider that answers nothing still hands over the bytes.
        }
        name?.takeIf { it.isNotBlank() }
            ?: uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() }
            ?: "file ${index + 1}"
    }
    val mimes = uris.map { uri -> runCatching { resolver.getType(uri) }.getOrNull() }
    return SharedFiles(uris, names, bytes, mimes)
}

/**
 * Writes shared `text` to a file named `{device}_{date and time}`. Text that is nothing but links
 * goes as a list of links (text/uri-list, `.uris`), which the receiving device offers to open; any
 * other text has no extension, its type, text/plain, going with it instead. Call it off the main
 * thread, and [forgetSharedText] when the screen it was shared to closes.
 */
fun writeSharedText(context: Context, text: String, device: String, now: java.util.Date = java.util.Date()): SharedFiles {
    val stamp = java.text.SimpleDateFormat("yyyy-MM-dd_HH-mm-ss", java.util.Locale.US).format(now)
    // Not a folder, and nothing a computer the file syncs to would refuse.
    val from = device.replace(Regex("""[\\/:*?"<>|\p{Cntrl}]"""), "-").trim().ifEmpty { "Android" }
    val links = uriListFromText(text)
    val name = if (links != null) "${from}_$stamp.$URI_LIST_EXTENSION" else "${from}_$stamp"
    val file = File(context.cacheDir, "$SHARED_TEXT_DIR/${UUID.randomUUID()}")
    try {
        file.parentFile?.mkdirs()
        file.writeText(links?.let(::encodeUriList) ?: text, Charsets.UTF_8)
    } catch (e: IOException) {
        file.delete()
        throw CoreException.Failed("file", "The text can't be saved: ${e.message}")
    }
    val mime = if (links != null) URI_LIST else TEXT_PLAIN
    return SharedFiles(listOf(Uri.fromFile(file)), listOf(name), file.length(), listOf(mime), text, links)
}

/** Deletes the file [writeSharedText] made for `shared`, if it made one. */
fun forgetSharedText(shared: SharedFiles?) {
    if (shared?.text == null) return
    shared.uris.forEach { uri -> uri.path?.let { File(it).delete() } }
}

/** Copies `source` to `dest`. Access to a shared file ends with the screen it was shared to. */
fun copyShared(context: Context, source: Uri, dest: File) {
    try {
        val input = context.contentResolver.openInputStream(source) ?: throw FileNotFoundException(source.toString())
        dest.parentFile?.mkdirs()
        input.use { from -> dest.outputStream().use { to -> from.copyTo(to) } }
    } catch (_: SecurityException) {
        throw CoreException.Failed("file", "Home Space isn't allowed to read this file.")
    } catch (e: IOException) {
        dest.delete()
        throw CoreException.Failed("file", "The file can't be read: ${e.message}")
    }
}

/** Copies the files to send into the app's cache. Call it off the main thread. */
fun prepareSend(context: Context, shared: SharedFiles): PreparedSend {
    val dir = File(context.cacheDir, "$SEND_DIR/${UUID.randomUUID()}")
    try {
        val files = shared.uris.mapIndexed { index, uri ->
            val copy = File(dir, index.toString())
            copyShared(context, uri, copy)
            SendFile(shared.names[index], copy.path, shared.mimes[index])
        }
        return PreparedSend(files, dir)
    } catch (e: CoreException) {
        dir.deleteRecursively()
        throw e
    }
}

/**
 * Puts the downloaded `files` of `dir` into the phone's Download folder, keeping their folders, and
 * answers where that is. Android gives a taken name a number.
 */
private fun exportToDownloads(context: Context, dir: File, files: List<String>): String {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
        // Before Android 10 the shared Download folder needs a storage permission the app doesn't ask for.
        val root = File(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), DOWNLOADS_FOLDER)
        for (file in files) {
            var target = File(root, file)
            var number = 2
            while (target.exists()) {
                target = File(target.parentFile, "${target.nameWithoutExtension} (${number++}).${target.extension}")
            }
            target.parentFile?.mkdirs()
            File(dir, file).copyTo(target)
        }
        return root.path
    }
    val resolver = context.contentResolver
    val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
    for (file in files) {
        val folder = file.substringBeforeLast('/', "")
        val name = file.substringAfterLast('/')
        val mime = MimeTypeMap.getSingleton()
            .getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: OCTET_STREAM
        val path = listOf(Environment.DIRECTORY_DOWNLOADS, DOWNLOADS_FOLDER, folder)
            .filter { it.isNotEmpty() }
            .joinToString("/")
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, mime)
            put(MediaStore.Downloads.RELATIVE_PATH, path)
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val target = resolver.insert(collection, values) ?: throw IOException("the Download folder can't be written")
        try {
            val output = resolver.openOutputStream(target) ?: throw FileNotFoundException(target.toString())
            output.use { sink -> File(dir, file).inputStream().use { it.copyTo(sink) } }
            values.clear()
            values.put(MediaStore.Downloads.IS_PENDING, 0)
            resolver.update(target, values, null, null)
        } catch (e: IOException) {
            resolver.delete(target, null, null)
            throw e
        }
    }
    return "${Environment.DIRECTORY_DOWNLOADS}/$DOWNLOADS_FOLDER"
}

/**
 * Tells the user about transfers waiting for this phone, once each. The server only pushes them
 * while the app is open, so this also runs after reconnecting and from background sync.
 *
 * With `quiet`, as while the Transfers screen is in front, new ones count as seen without a
 * notification. Without the permission to post, they count as seen too, so that granting it later
 * doesn't bring a pile of old ones.
 */
// The permission is checked by needsNotificationPermission, which lint can't see into.
@SuppressLint("MissingPermission")
fun notifyIncoming(context: Context, received: List<TransferRow>, quiet: Boolean = false) {
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    val waiting = received.filter { it.thisDevice && it.status == TransferStatus.READY }
    val notified = prefs.getStringSet(NOTIFIED, emptySet()).orEmpty()
    val manager = NotificationManagerCompat.from(context)
    // Those no longer waiting were dealt with, here or on another device.
    (notified - waiting.map { it.id }.toSet()).forEach { manager.cancel(it, INCOMING_NOTIFICATION) }
    prefs.edit().putStringSet(NOTIFIED, waiting.map { it.id }.toSet()).apply()
    val fresh = waiting.filter { it.id !in notified }
    if (fresh.isEmpty() || quiet || needsNotificationPermission(context)) return
    manager.createNotificationChannel(
        NotificationChannelCompat.Builder(INCOMING_CHANNEL, NotificationManagerCompat.IMPORTANCE_HIGH)
            .setName(INCOMING_CHANNEL_NAME)
            .setDescription(INCOMING_CHANNEL_DESCRIPTION)
            .build(),
    )
    val open = Intent(context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        .putExtra(EXTRA_SHOW_TRANSFERS, true)
    // One for all of them: each opens the same tab.
    val tap = PendingIntent.getActivity(
        context, OPEN_TRANSFERS_REQUEST, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    for (row in fresh) {
        val notification = NotificationCompat.Builder(context, INCOMING_CHANNEL)
            .setSmallIcon(R.drawable.ic_download)
            .setContentTitle(row.noticeTitle)
            .setContentText(row.noticeDetail)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .apply { if (row.canOpen) addAction(openAction(context, row)) }
            .build()
        manager.notify(row.id, INCOMING_NOTIFICATION, notification)
    }
}

/**
 * "Open" on a received link's notification. It goes through the app, which shows the link and asks
 * before anything opens; a notification can't start another app's activity on its own anyway.
 */
private fun openAction(context: Context, row: TransferRow): NotificationCompat.Action {
    val open = Intent(context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        .putExtra(EXTRA_SHOW_TRANSFERS, true)
        .putExtra(EXTRA_OPEN_TRANSFER, row.id)
    // One per transfer, told apart by the request code: the extras alone don't make them differ.
    val intent = PendingIntent.getActivity(
        context, row.id.hashCode(), open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    return NotificationCompat.Action.Builder(R.drawable.ic_open_in_new, "Open", intent).build()
}

/** Takes down the notifications of received transfers and forgets them, as on signing out. */
fun forgetIncomingNotices(context: Context) {
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    val manager = NotificationManagerCompat.from(context)
    prefs.getStringSet(NOTIFIED, emptySet()).orEmpty().forEach { manager.cancel(it, INCOMING_NOTIFICATION) }
    prefs.edit().remove(NOTIFIED).apply()
}

/**
 * Whether to ask for the permission to post notifications now: Android 13 or later, not granted,
 * and not asked before by [markNotificationsAsked]. Asked once, so that a "no" is not met with the
 * same question on every start.
 */
fun shouldAskForNotifications(context: Context): Boolean =
    needsNotificationPermission(context) &&
        !context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(ASKED_NOTIFICATIONS, false)

/** Remembers that the permission to post notifications was asked for. */
fun markNotificationsAsked(context: Context) {
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(ASKED_NOTIFICATIONS, true).apply()
}

/** Keeps the process going, with a notification, until the sends from this phone are done. */
class SendWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        try {
            setForeground(getForegroundInfo())
        } catch (_: IllegalStateException) {
            // Android doesn't allow it right now. The send still runs while the process lives.
        }
        TransferCenter.sends.first { it == 0 }
        return Result.success()
    }

    override suspend fun getForegroundInfo(): ForegroundInfo {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(SENDING_CHANNEL, "Sending to a device", NotificationManager.IMPORTANCE_LOW),
        )
        val notification = Notification.Builder(applicationContext, SENDING_CHANNEL)
            .setContentTitle("Sending files to a device")
            .setSmallIcon(R.drawable.ic_send)
            .setOngoing(true)
            .setProgress(0, 0, true)
            .build()
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ForegroundInfo(SENDING_NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(SENDING_NOTIFICATION, notification)
        }
    }
}

private fun keepSending(context: Context) {
    val request = OneTimeWorkRequestBuilder<SendWorker>()
        .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
        .build()
    WorkManager.getInstance(context).enqueueUniqueWork(SEND_WORK, ExistingWorkPolicy.KEEP, request)
}
