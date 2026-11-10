package app.homespace.android

import android.app.Application
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.widget.Toast
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.DeviceIdentity
import uniffi.homespace_core.DownloadListener
import uniffi.homespace_core.Entry
import uniffi.homespace_core.EntryKind
import uniffi.homespace_core.EventStream
import uniffi.homespace_core.EventsListener
import uniffi.homespace_core.LiveEvent
import uniffi.homespace_core.QueuedUpload
import uniffi.homespace_core.SessionListener
import uniffi.homespace_core.SignInOptions
import uniffi.homespace_core.SignedInUser
import uniffi.homespace_core.TransferRow
import uniffi.homespace_core.normalizeServerUrl

/** The server on the host machine, as seen from the emulator. */
private const val DEFAULT_SERVER = "http://10.0.2.2:8080"
private const val PROBE_DELAY_MILLIS = 600L
/** How long live changes are gathered before the folder on screen is reloaded once for them. */
private const val LIVE_RELOAD_DELAY_MILLIS = 300L

sealed interface Session {
    /** Trying a saved session; `error` is set when that failed but the session may still work. */
    data class Resuming(val server: String, val error: String? = null) : Session
    data object SignedOut : Session
    data class SignedIn(val client: CoreClient, val user: SignedInUser) : Session
}

/** What the server said about how to sign in. */
sealed interface Probe {
    data object Checking : Probe
    data class Ready(val options: SignInOptions) : Probe
    data class Failed(val message: String) : Probe
}

/** A folder on the way down from Home; `id` is null for Home itself. */
data class Crumb(val id: String?, val name: String)

/** A downloaded file waiting to be shown. */
data class OpenRequest(val path: String, val mime: String, val edit: Boolean = false)

/** A copy with edits not on the server yet, waiting for the user to say whether it may go. */
data class RemovalWarning(val entry: Entry, val names: List<String>)

/**
 * The session and the sign-in form. It outlives the activity, so the signed-in client survives
 * rotation, and a saved token is only ever spent once: replaying a rotated refresh token ends
 * the session.
 */
class SessionViewModel(application: Application) : AndroidViewModel(application) {
    private val store = SessionStore(application)
    private var probeJob: Job? = null

    var session by mutableStateOf<Session>(Session.SignedOut)
        private set
    var server by mutableStateOf(store.lastServer ?: DEFAULT_SERVER)
        private set
    var probe by mutableStateOf<Probe>(Probe.Checking)
        private set
    var pendingSso by mutableStateOf(store.loadPendingSso())
        private set
    /** The address of the sign-in the browser is on, to reopen it; gone if the app was killed. */
    var pendingSsoUrl by mutableStateOf<String?>(null)
        private set
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    var username by mutableStateOf("")
    var password by mutableStateOf("")

    var themeMode by mutableStateOf(store.themeMode)
        private set
    var useSystemColours by mutableStateOf(store.useSystemColours)
        private set

    /** Where the folder browser is, kept here so that rotating the phone doesn't reset it. */
    val path = mutableStateListOf<Crumb>()

    /** How far along each running download is, from 0 to 1, whichever folder is on screen. */
    val downloads = mutableStateMapOf<String, Float>()

    /** The files picked to upload, in the order they go up. */
    val uploads = mutableStateListOf<QueuedUpload>()
    private var uploadWatch: Job? = null

    /** What was sent to the user, and what the user sent, the newest first. */
    val received = mutableStateListOf<TransferRow>()
    val sent = mutableStateListOf<TransferRow>()

    /** Why the transfers couldn't be listed, such as a server from before it had them. */
    var transfersError by mutableStateOf<String?>(null)
        private set

    /** Set to bring the Transfers tab to the front, as a transfer's notification does. */
    var showTransfers by mutableStateOf(false)

    /**
     * Set by the signed-in screen while its Transfers tab is the one shown. A transfer that arrives
     * then, with the app in front, is in plain sight and gets no notification.
     */
    var transfersOnScreen = false
    private var transferWatch: Job? = null

    /** The folder on screen is reloaded whenever this changes. */
    var reloadKey by mutableIntStateOf(0)
        private set

    /** The user's devices are reloaded whenever this changes, such as when one comes online. */
    var devicesKey by mutableIntStateOf(0)
        private set

    private val _liveEvents = MutableSharedFlow<LiveEvent>(extraBufferCapacity = 64)

    /** What the server pushes while the app is in front, for the screens to pick what to reload. */
    val liveEvents: SharedFlow<LiveEvent> = _liveEvents

    private var pendingReload: Job? = null

    /** A finished download for the screen to open. */
    /** The copy waiting for the user to say whether its unsent edits may go. */
    var removalWarning by mutableStateOf<RemovalWarning?>(null)

    /** The names of the files whose unsent edits signing out would lose, while asking. */
    var signOutWarning by mutableStateOf<List<String>?>(null)

    var pendingOpen by mutableStateOf<OpenRequest?>(null)
        private set

    /** Set by the screen: a file that finishes downloading in the background isn't opened. */
    var foreground = false

    /** The file whose destination the system's save screen is asking for. */
    var pendingSave: Entry? = null

    /** "Sync only on Wi-Fi" is on, and the phone isn't on it. */
    var waitingForWifi by mutableStateOf(false)
        private set

    private var sync: ForegroundSync? = null
    private var events: EventStream? = null
    private var appInFront = false

    // Only the file tapped last opens when it finishes.
    private var lastTapped: String? = null

    init {
        val saved = store.loadSession()
        val pending = pendingSso
        if (saved != null) {
            session = Session.Resuming(saved.server)
            server = saved.server
            resume(saved)
        } else {
            if (pending != null) server = pending.server
            probeNow()
        }
    }

    fun retryResume() {
        val saved = store.loadSession()
        if (saved == null) {
            backToForm()
        } else {
            session = Session.Resuming(saved.server)
            resume(saved)
        }
    }

    private fun resume(saved: SavedSession) {
        viewModelScope.launch {
            try {
                // Finds the client if background work or an earlier activity already restored it.
                val active = ClientHolder.resume(getApplication())
                if (active == null) backToForm() else enter(active)
            } catch (e: CoreException) {
                if (e is CoreException.Unauthorized) {
                    backToForm()
                } else {
                    session = Session.Resuming(saved.server, e.signInMessage())
                }
            }
        }
    }

    fun onServerChanged(text: String) {
        server = text
        error = null
        probe = Probe.Checking
        probeJob?.cancel()
        probeJob = viewModelScope.launch {
            delay(PROBE_DELAY_MILLIS)
            check()
        }
    }

    /** Asks the server again straight away, such as after a failure. */
    fun probeNow() {
        probe = Probe.Checking
        probeJob?.cancel()
        probeJob = viewModelScope.launch { check() }
    }

    // Replacing the job cancels the check before it, so a late answer can't overwrite a new one.
    private suspend fun check() {
        probe = try {
            val answer = CoreClient(normalizeServerUrl(server)).use { it.providers() }
            Probe.Ready(answer)
        } catch (e: CoreException) {
            Probe.Failed(e.signInMessage())
        }
    }

    fun signInWithPassword() {
        val attempt = password
        password = ""
        error = null
        busy = true
        viewModelScope.launch {
            try {
                val client = newClient(normalizeServerUrl(server))
                try {
                    adoptAndEnter(client, client.login(username.trim(), attempt))
                } catch (e: CoreException) {
                    client.close()
                    throw e
                }
            } catch (e: CoreException) {
                error = e.signInMessage()
            } finally {
                busy = false
            }
        }
    }

    fun changeThemeMode(mode: ThemeMode) {
        store.themeMode = mode
        themeMode = mode
    }

    fun changeUseSystemColours(use: Boolean) {
        store.useSystemColours = use
        useSystemColours = use
    }

    /** Starts a single sign-on and returns the URL to open in a browser. */
    fun beginSso(name: String): String? {
        error = null
        return try {
            val address = normalizeServerUrl(server)
            val request = CoreClient(address).use { it.beginSso() }
            val pending = PendingSso(address, request.verifier, name)
            if (store.savePendingSso(pending)) {
                pendingSso = pending
                pendingSsoUrl = request.url
                request.url
            } else {
                error = SIGN_IN_FAILED
                null
            }
        } catch (e: CoreException) {
            error = e.signInMessage()
            null
        }
    }

    fun cancelSso() {
        store.forgetPendingSso()
        pendingSso = null
        pendingSsoUrl = null
    }

    fun onBrowserFailed() {
        cancelSso()
        error = "No browser is available to sign in with."
    }

    /** The browser came back. Ignored unless a sign-in is waiting, so a stray link does nothing. */
    fun onSsoCallback(url: String) {
        val pending = store.loadPendingSso() ?: return
        cancelSso()
        server = pending.server
        error = null
        busy = true
        viewModelScope.launch {
            val client = newClient(pending.server)
            try {
                adoptAndEnter(client, client.finishSso(url, pending.verifier))
            } catch (e: CoreException) {
                client.close()
                error = e.signInMessage()
            } finally {
                busy = false
            }
        }
    }

    fun reload() {
        reloadKey++
    }

    /** Reloads the folder on screen shortly, once for however many changes arrive meanwhile. */
    fun reloadSoon() {
        if (pendingReload?.isActive == true) return
        pendingReload = viewModelScope.launch {
            delay(LIVE_RELOAD_DELAY_MILLIS)
            reload()
        }
    }

    fun onOpened() {
        pendingOpen = null
    }

    /** Shows a file: at once if the copy here is current, else after downloading it. */
    fun openFile(session: Session.SignedIn, entry: Entry, edit: Boolean = false) {
        lastTapped = entry.id
        if (entry.id in downloads) return
        downloads[entry.id] = 0f
        viewModelScope.launch {
            try {
                val path = session.client.openCopy(entry.id, progressListener())
                if (lastTapped == entry.id && foreground) {
                    pendingOpen = OpenRequest(path, entry.mime, edit)
                }
                reload()
            } catch (e: CoreException) {
                onDownloadFailed(session, entry, e)
            } finally {
                downloads.remove(entry.id)
            }
        }
    }

    /** Saves a file where the user chose; the document the system created goes if that fails. */
    fun saveFile(session: Session.SignedIn, entry: Entry, destination: Uri) {
        val resolver = getApplication<Application>().contentResolver
        val temp = File(getApplication<Application>().cacheDir, "save/${entry.id}")
        downloads[entry.id] = 0f
        viewModelScope.launch {
            var saved = false
            try {
                session.client.saveCopy(entry.id, temp.path, progressListener())
                withContext(Dispatchers.IO) {
                    val output = resolver.openOutputStream(destination, "wt")
                        ?: throw FileNotFoundException(destination.toString())
                    output.use { sink -> temp.inputStream().use { it.copyTo(sink) } }
                }
                saved = true
                toast("Saved “${entry.name}”")
            } catch (e: CoreException) {
                onDownloadFailed(session, entry, e)
            } catch (e: IOException) {
                toast("Could not save “${entry.name}”: ${e.message}")
            } finally {
                downloads.remove(entry.id)
                withContext(Dispatchers.IO) {
                    temp.delete()
                    if (!saved) {
                        runCatching { DocumentsContract.deleteDocument(resolver, destination) }
                    }
                }
            }
        }
    }

    /** Keeps a file or folder on this phone and in sync. */
    fun makeAvailableOffline(session: Session.SignedIn, entry: Entry) {
        viewModelScope.launch {
            try {
                session.client.makeAvailableOffline(entry.id)
            } catch (e: CoreException) {
                toast("Failed: ${e.fileMessage()}")
            }
            reload()
        }
    }

    /** Deletes this phone's copy of a file or folder, after a warning if that loses edits. */
    fun removeOfflineCopy(session: Session.SignedIn, entry: Entry, confirmed: Boolean = false) {
        removalWarning = null
        viewModelScope.launch {
            try {
                val unsent = if (confirmed) emptyList() else session.client.unsentEdits(entry.id)
                if (unsent.isEmpty()) {
                    session.client.removeOfflineCopy(entry.id)
                } else {
                    removalWarning = RemovalWarning(entry, unsent)
                }
            } catch (e: CoreException) {
                toast("Failed: ${e.fileMessage()}")
            }
            reload()
        }
    }

    /** Signs out, after a warning if that loses edits. */
    fun requestSignOut(session: Session.SignedIn) {
        viewModelScope.launch {
            val unsent = runCatching { session.client.unsentEdits(null) }.getOrDefault(emptyList())
            if (unsent.isEmpty()) signOut() else signOutWarning = unsent
        }
    }

    private fun progressListener() = object : DownloadListener {
        override fun progress(id: String, done: Long, total: Long) {
            val fraction = if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else 1f
            viewModelScope.launch {
                // A report can arrive after the download has ended.
                if (id in downloads) downloads[id] = fraction
            }
        }
    }

    private fun onDownloadFailed(
        failed: Session.SignedIn,
        entry: Entry,
        error: CoreException,
    ) {
        when {
            error is CoreException.Cancelled -> Unit
            error !is CoreException.Unauthorized ->
                toast("Could not download “${entry.name}”: ${error.fileMessage()}")
            session === failed -> onSessionEnded()
        }
    }

    /** Lists the transfers again, and tells the user about any new one for this phone. */
    fun loadTransfers() {
        val current = session as? Session.SignedIn ?: return
        viewModelScope.launch {
            try {
                val lists = current.client.transfers()
                if (session !== current) return@launch
                received.clear()
                received.addAll(lists.received)
                sent.clear()
                sent.addAll(lists.sent)
                transfersError = null
                notifyIncoming(getApplication(), lists.received, quiet = appInFront && transfersOnScreen)
            } catch (_: CoreException.Unauthorized) {
                if (session === current) onSessionEnded()
            } catch (e: CoreException) {
                transfersError = e.fileMessage()
            }
        }
    }

    /** Saves a received transfer to the phone's Download folder. */
    fun downloadTransfer(session: Session.SignedIn, row: TransferRow) {
        TransferCenter.download(getApplication(), session.client, row) {
            if (this.session === session) onSessionEnded()
        }
    }

    fun stopTransferDownload(session: Session.SignedIn, row: TransferRow) {
        runCatching { session.client.stopTransferDownload(row.id) }
    }

    fun declineTransfer(session: Session.SignedIn, row: TransferRow) =
        changeTransfer(session) { session.client.declineTransfer(row.id) }

    /** Takes back a transfer the user sent. */
    fun cancelTransfer(session: Session.SignedIn, row: TransferRow) =
        changeTransfer(session) { session.client.cancelTransfer(row.id) }

    private fun changeTransfer(current: Session.SignedIn, change: suspend () -> Unit) {
        viewModelScope.launch {
            try {
                change()
            } catch (_: CoreException.Unauthorized) {
                if (session === current) onSessionEnded()
            } catch (e: CoreException) {
                toast("Failed: ${e.fileMessage()}")
            }
            loadTransfers()
        }
    }

    private fun watchTransfers() {
        transferWatch?.cancel()
        // Collecting starts with the current value, which lists them for the first time.
        transferWatch = viewModelScope.launch {
            TransferCenter.changes.collect { loadTransfers() }
        }
    }

    private fun forgetTransfers() {
        transferWatch?.cancel()
        transferWatch = null
        received.clear()
        sent.clear()
        transfersError = null
        forgetIncomingNotices(getApplication())
    }

    /**
     * Adds the files to the uploads; they go up one at a time, whichever folder is on screen, and
     * carry on when the app next opens. Access to each is kept until it has been copied.
     */
    fun uploadFiles(files: List<Uri>, folder: Crumb) {
        val active = ClientHolder.active ?: return
        val resolver = getApplication<Application>().contentResolver
        try {
            for (uri in files) {
                try {
                    resolver.takePersistableUriPermission(uri, READ_GRANT)
                } catch (_: SecurityException) {
                    // Not every provider offers it. It works until the app ends.
                }
                active.uploads.add(folder.id, folder.name, displayName(uri), uri.toString())
            }
        } catch (e: CoreException) {
            toast("The files can't be added: ${e.fileMessage()}")
        }
        sync?.runUploads()
    }

    fun retryUpload(upload: QueuedUpload) {
        val active = ClientHolder.active ?: return
        runCatching { active.uploads.retry(upload.key) }
        sync?.runUploads()
    }

    /** Gives up on an upload: the row goes now, and a running one stops at its next chunk. */
    fun cancelUpload(upload: QueuedUpload) {
        val active = ClientHolder.active ?: return
        runCatching { active.uploads.cancel(upload.key) }
    }

    private fun watchUploads(active: ActiveSession) {
        uploadWatch?.cancel()
        uploadWatch = viewModelScope.launch {
            ClientHolder.uploadChanges.collect {
                val items = withContext(Dispatchers.IO) {
                    runCatching { active.uploads.items() }.getOrNull()
                } ?: return@collect
                val arrived = uploads.any { old -> items.none { it.key == old.key } }
                uploads.clear()
                uploads.addAll(items)
                if (arrived) reload()
            }
        }
    }

    private fun forgetUploads() {
        uploadWatch?.cancel()
        uploadWatch = null
        uploads.clear()
    }

    private fun displayName(uri: Uri): String {
        val resolver = getApplication<Application>().contentResolver
        val name = try {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0) else null
            }
        } catch (_: RuntimeException) {
            null
        }
        return name?.takeIf { it.isNotBlank() } ?: uri.lastPathSegment?.substringAfterLast('/') ?: "file"
    }

    /**
     * Shows the folder of a search result: a folder itself, or the one holding a file. False when
     * its place can't be found, which has been said.
     */
    suspend fun showInFolder(session: Session.SignedIn, entry: Entry): Boolean {
        val above = try {
            session.client.ancestors(entry.id)
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
            return false
        } catch (e: CoreException) {
            toast(e.fileMessage())
            return false
        }
        path.clear()
        path.add(Crumb(null, "Home"))
        above.mapTo(path) { Crumb(it.id, it.name) }
        if (entry.kind == EntryKind.FOLDER) path.add(Crumb(entry.id, entry.name))
        return true
    }

    private fun toast(message: String) {
        Toast.makeText(getApplication(), message, Toast.LENGTH_LONG).show()
    }

    fun signOut() {
        signOutWarning = null
        val current = session
        val active = ClientHolder.active
        store.forgetSession()
        downloads.clear()
        forgetUploads()
        forgetTransfers()
        endSync()
        backToForm()
        if (current is Session.SignedIn) {
            viewModelScope.launch {
                // Already signed out here; this only revokes the token on the server. Auto-upload
                // goes first, since cancelling its uploads takes the session.
                try {
                    active?.uploads?.wipe()
                    active?.autoUpload?.wipe()
                    current.client.logout()
                } catch (_: CoreException) {
                } finally {
                    current.client.close()
                }
            }
        }
    }

    /** The server ended the session while the user was browsing. */
    fun onSessionEnded() {
        val current = session
        val active = ClientHolder.active
        store.forgetSession()
        downloads.clear()
        forgetUploads()
        forgetTransfers()
        endSync()
        backToForm()
        if (current is Session.SignedIn) {
            viewModelScope.launch {
                try {
                    active?.uploads?.wipe()
                    active?.autoUpload?.wipe()
                } catch (_: CoreException) {
                } finally {
                    current.client.close()
                }
            }
        }
    }

    // The client stays with `ClientHolder`, for the next activity or background work.
    override fun onCleared() {
        sync?.stop()
        stopEvents()
    }

    /** Called as the app comes to the front and leaves it. */
    fun onForegroundChanged(inFront: Boolean) {
        appInFront = inFront
        ClientHolder.appInFront = inFront
        if (inFront) {
            clearUploadFailureNotice(getApplication())
            sync?.start()
            startEvents()
        } else {
            sync?.stop()
            stopEvents()
        }
    }

    /**
     * Opens the live events socket of the signed-in session. Only while the app is in front: in
     * the background the system cuts the network and the socket would die unnoticed.
     */
    private fun startEvents() {
        val current = session as? Session.SignedIn ?: return
        if (events != null || !appInFront) return
        lateinit var stream: EventStream
        stream = current.client.listenEvents(object : EventsListener {
            override fun event(event: LiveEvent) {
                viewModelScope.launch {
                    if (event is LiveEvent.DevicesChanged || event is LiveEvent.Resync) {
                        devicesKey++
                        TransferCenter.devicesChanged()
                    }
                    if (event is LiveEvent.TransfersChanged || event is LiveEvent.Resync) loadTransfers()
                    _liveEvents.emit(event)
                }
            }

            override fun sessionEnded() {
                viewModelScope.launch {
                    if (events === stream && session === current) onSessionEnded()
                }
            }
        })
        events = stream
    }

    private fun stopEvents() {
        events?.let {
            it.stop()
            it.close()
        }
        events = null
    }

    /** Tries what can be tried at once, such as after the user asked. */
    fun syncNow() {
        sync?.syncNow()
    }

    private fun endSync() {
        stopEvents()
        sync?.stop()
        sync = null
        cancelBackgroundSync(getApplication())
        cancelUploadWork(getApplication())
        // Only auto-upload keeps access to folders, and it forgets its sources now.
        val resolver = getApplication<Application>().contentResolver
        resolver.persistedUriPermissions.forEach {
            runCatching { resolver.releasePersistableUriPermission(it.uri, READ_GRANT) }
        }
        ClientHolder.active?.let(ClientHolder::release)
    }

    private fun adoptAndEnter(client: CoreClient, user: SignedInUser) {
        enter(ClientHolder.adopt(getApplication(), client, user))
    }

    private fun enter(active: ActiveSession) {
        username = ""
        error = null
        path.clear()
        path.add(Crumb(null, "Home"))
        session = Session.SignedIn(active.client, active.user)
        sync = ForegroundSync(getApplication(), active, viewModelScope, ::onSessionEnded) {
            viewModelScope.launch { waitingForWifi = !it }
        }.also {
            if (appInFront) it.start()
        }
        startEvents()
        watchUploads(active)
        watchTransfers()
        TransferCenter.sweep(getApplication())
        runCatching { applyBackgroundSetting(getApplication(), active.autoUpload.settings()) }
    }

    private fun backToForm() {
        session = Session.SignedOut
        path.clear()
        probeNow()
    }

    private fun newClient(address: String): CoreClient =
        ClientHolder.newClient(getApplication(), address)
}

private const val READ_GRANT = Intent.FLAG_GRANT_READ_URI_PERMISSION

private const val SIGN_IN_FAILED = "Signing in failed. Please try again."

/** The sentence to show when a file operation fails; the generated `message` lists raw fields. */
internal fun CoreException.fileMessage(): String = when (this) {
    is CoreException.Failed -> detail.replaceFirstChar { it.uppercase() }
    is CoreException.Unreachable -> "Could not reach the server"
    else -> message.orEmpty()
}

/** The sentence to show for a failure, matching the web client's. */
private fun CoreException.signInMessage(): String = when (this) {
    is CoreException.InvalidCredentials -> "Invalid username or password"
    is CoreException.SsoExpired -> "The sign-in expired. Please try again."
    is CoreException.SsoDenied -> "You are not allowed to sign in with this account."
    is CoreException.SsoFailed -> SIGN_IN_FAILED
    is CoreException.InvalidServerAddress ->
        "The server address must start with http:// or https://"
    is CoreException.UnprotectedServerAddress ->
        "Use https:// for a server outside your home network"
    is CoreException.Unreachable -> "Could not reach the server"
    is CoreException.Failed -> detail
    is CoreException.Unauthorized -> "Your session has ended. Please sign in again."
    is CoreException.InvalidId, is CoreException.Cancelled -> SIGN_IN_FAILED
}
