package app.homespace.android

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreException

/** What the share screen is showing. */
private enum class ShareStep { CHOOSE, SAVE, SEND }

/** What is known about who is signed in. */
private sealed interface ShareSession {
    data object Loading : ShareSession
    data object SignedOut : ShareSession
    data class Failed(val message: String) : ShareSession
    data class Ready(val active: ActiveSession) : ShareSession
}

/**
 * Takes files another app shares, such as a mail attachment, or text, such as a link to a video:
 * Save puts them into a folder of the user's Home Space, Send sends them to a device. Text goes as
 * a file of its own, named after this phone and the time.
 */
class ShareActivity : ComponentActivity() {
    /** The file shared text was written to; it goes with this screen. */
    private var textFile: SharedFiles? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val uris = sharedUris(intent)
        // A file wins over the text that comes with it, which is a caption at most.
        val text = if (uris.isEmpty()) sharedText(intent) else null
        if (uris.isEmpty() && text == null) {
            Toast.makeText(this, "There is nothing to save or send.", Toast.LENGTH_LONG).show()
            finish()
            return
        }
        val store = SessionStore(this)
        setContent {
            val dark = when (store.themeMode) {
                ThemeMode.SYSTEM -> isSystemInDarkTheme()
                ThemeMode.LIGHT -> false
                ThemeMode.DARK -> true
            }
            HomeSpaceTheme(dark = dark, useSystemColours = store.useSystemColours) {
                Surface { ShareScreen(uris, text) }
            }
        }
    }

    override fun onDestroy() {
        // Whatever goes up or out was copied by now.
        forgetSharedText(textFile)
        super.onDestroy()
    }

    @Composable
    private fun ShareScreen(uris: List<Uri>, text: String?) {
        var session by remember { mutableStateOf<ShareSession>(ShareSession.Loading) }
        var shared by remember { mutableStateOf<SharedFiles?>(null) }
        var step by rememberSaveable { mutableStateOf(ShareStep.CHOOSE) }
        LaunchedEffect(Unit) {
            session = try {
                shared = withContext(Dispatchers.IO) {
                    if (text != null) {
                        writeSharedText(applicationContext, text, Build.MODEL).also { textFile = it }
                    } else {
                        describeShared(applicationContext, uris)
                    }
                }
                ClientHolder.resume(applicationContext)?.let(ShareSession::Ready) ?: ShareSession.SignedOut
            } catch (_: CoreException.Unauthorized) {
                ShareSession.SignedOut
            } catch (e: CoreException) {
                ShareSession.Failed(e.fileMessage())
            }
        }
        val onSessionEnded = { session = ShareSession.SignedOut }
        val files = shared
        val ready = session as? ShareSession.Ready
        if (ready != null && files != null && step == ShareStep.SEND) {
            // Not a dialog: it is the screen itself, and back returns to the choice.
            androidx.activity.compose.BackHandler { step = ShareStep.CHOOSE }
            SendToDevice(
                summary = sharedSummary(files),
                client = ready.active.client,
                send = { target ->
                    // Copied first, because access to the files ends with this screen.
                    val prepared = withContext(Dispatchers.IO) { prepareSend(applicationContext, files) }
                    TransferCenter.send(applicationContext, ready.active.client, prepared, target)
                },
                onClose = { step = ShareStep.CHOOSE },
                onSent = { target ->
                    toast("Sending to ${target.recipient}")
                    finish()
                },
                onSessionEnded = onSessionEnded,
            )
            return
        }
        ChooseScreen(session, files, onSave = { step = ShareStep.SAVE }, onSend = { step = ShareStep.SEND })
        if (ready != null && files != null && step == ShareStep.SAVE) {
            SaveToFolderDialog(
                summary = sharedSummary(files),
                client = ready.active.client,
                save = { folder, name -> withContext(Dispatchers.IO) { queueUploads(ready.active, files, folder, name) } },
                onDismiss = { step = ShareStep.CHOOSE },
                onSaved = { folder ->
                    toast(
                        when {
                            files.text != null -> "Saving the text to $folder"
                            files.uris.size == 1 -> "Uploading to $folder"
                            else -> "Uploading ${files.uris.size} files to $folder"
                        },
                    )
                    finish()
                },
                onSessionEnded = onSessionEnded,
            )
        }
    }

    /**
     * Puts the files in the upload queue with a copy each, since access to them ends with this
     * screen, and starts the queue. They then go up like files picked in the app.
     */
    private fun queueUploads(active: ActiveSession, files: SharedFiles, folder: String?, folderName: String) {
        val queue = active.uploads
        val added = mutableListOf<String>()
        try {
            files.uris.forEachIndexed { index, uri ->
                val key = queue.add(folder, folderName, files.names[index], null, files.mimes[index])
                added += key
                copyShared(applicationContext, uri, java.io.File(queue.copyPath(key)))
                queue.ready(key, null)
            }
        } catch (e: CoreException) {
            // All or nothing: a share that half arrived would be hard to tell from a whole one.
            added.forEach { runCatching { queue.cancel(it) } }
            throw e
        }
        scheduleUploads(applicationContext)
        queue.start()
    }

    private fun toast(message: String) {
        Toast.makeText(this, message, Toast.LENGTH_LONG).show()
    }

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun ChooseScreen(
        session: ShareSession,
        shared: SharedFiles?,
        onSave: () -> Unit,
        onSend: () -> Unit,
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text("Home Space") },
                    navigationIcon = {
                        IconButton(onClick = ::finish) {
                            Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                        }
                    },
                )
            },
        ) { padding ->
            Column(modifier = Modifier.padding(padding).fillMaxSize()) {
                shared?.let { SharedCard(it) }
                when (session) {
                    ShareSession.Loading -> Box(
                        modifier = Modifier.fillMaxWidth().padding(32.dp),
                        contentAlignment = Alignment.Center,
                    ) { CircularProgressIndicator() }
                    ShareSession.SignedOut -> Notice(
                        "Sign in to Home Space to save or send ${if (shared?.text != null) "it" else "files"}.",
                        "Open Home Space",
                    ) {
                        startActivity(Intent(this@ShareActivity, MainActivity::class.java))
                        finish()
                    }
                    is ShareSession.Failed -> Notice(session.message, "Close", ::finish)
                    is ShareSession.Ready -> {
                        SectionHeader("What to do with ${if ((shared?.uris?.size ?: 1) == 1) "it" else "them"}")
                        HorizontalDivider()
                        ChoiceRow(
                            R.drawable.ic_drive_file_move,
                            "Save",
                            "Pick a folder in your Home Space",
                            enabled = shared != null,
                            onClick = onSave,
                        )
                        ChoiceRow(
                            R.drawable.ic_send,
                            "Send",
                            "To one of your devices, or another user’s",
                            enabled = shared != null,
                            onClick = onSend,
                        )
                    }
                }
            }
        }
    }

    /** What was shared. */
    @Composable
    private fun SharedCard(shared: SharedFiles) {
        Card(
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest),
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        ) {
            Row(
                modifier = Modifier.padding(16.dp),
                horizontalArrangement = Arrangement.spacedBy(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    painterResource(if (shared.text != null) R.drawable.ic_node_description else R.drawable.ic_upload_file),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    sharedSummary(shared),
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }

    @Composable
    private fun Notice(message: String, button: String, onClick: () -> Unit) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(message, style = MaterialTheme.typography.bodyLarge)
            Button(onClick = onClick) { Text(button) }
        }
    }

    @Composable
    private fun ChoiceRow(icon: Int, title: String, detail: String, enabled: Boolean, onClick: () -> Unit) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(enabled = enabled, onClick = onClick)
                .heightIn(min = ROW_MIN_HEIGHT)
                .padding(horizontal = 16.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconTile(icon, MaterialTheme.colorScheme.primaryContainer, MaterialTheme.colorScheme.onPrimaryContainer)
            Column(modifier = Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.bodyLarge)
                Text(
                    detail,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Chevron()
        }
    }
}

/** The files of a share intent: one for `SEND`, several for `SEND_MULTIPLE`. */
private fun sharedUris(intent: Intent?): List<Uri> {
    if (intent == null) return emptyList()
    @Suppress("DEPRECATION")
    val uris = when (intent.action) {
        Intent.ACTION_SEND ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                listOfNotNull(intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java))
            } else {
                listOfNotNull(intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
            }
        Intent.ACTION_SEND_MULTIPLE ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            } else {
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            }
        else -> emptyList()
    }
    // Only content the sharing app hands over; a file path could point into this app's own data.
    return uris.filter { it.scheme == "content" }
}

/** The text of a `SEND` share, such as a link or a note, if it has any. */
private fun sharedText(intent: Intent?): String? {
    if (intent?.action != Intent.ACTION_SEND) return null
    return intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf { it.isNotBlank() }
}
