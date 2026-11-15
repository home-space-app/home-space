package app.homespace.android

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContract
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.DownloadListener
import uniffi.homespace_core.EntryKind
import uniffi.homespace_core.SharedEntry
import uniffi.homespace_core.viewerFor
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException

private const val SHARED_WITH_ME = "Shared with me"
private const val VIEW_ONLY = "View only"
private const val OCTET_STREAM_TYPE = "application/octet-stream"
private const val NOTHING_SHARED =
    "Nothing is shared with you yet. Files and folders other people share with you show up here."
private const val EMPTY_FOLDER = "This folder is empty."
private const val SEPARATOR = " · "

/** What other users shared with the user: read-only, opened, saved on the phone or copied to Files. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SharedScreen(
    session: Session.SignedIn,
    model: SessionViewModel,
    navigationBar: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // The folders opened, from a shared one down to the one shown; empty for everything shared.
    var trail by remember { mutableStateOf(emptyList<SharedEntry>()) }
    var entries by remember { mutableStateOf<List<SharedEntry>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var reloads by remember { mutableStateOf(0) }
    var acting by remember { mutableStateOf<SharedEntry?>(null) }
    var saving by remember { mutableStateOf<SharedEntry?>(null) }
    var pendingDownload by remember { mutableStateOf<SharedEntry?>(null) }
    var downloading by remember { mutableStateOf(false) }
    // The file open in its built-in viewer, with the files beside it.
    var viewing by remember { mutableStateOf<SharedEntry?>(null) }

    LaunchedEffect(trail, reloads) {
        entries = null
        error = null
        try {
            val folder = trail.lastOrNull()
            entries = if (folder == null) {
                session.client.sharedWithMe()
            } else {
                session.client.sharedChildren(folder)
            }
        } catch (e: CoreException) {
            if (e is CoreException.Unauthorized) model.onSessionEnded() else error = e.fileMessage()
            entries = emptyList()
        }
    }
    BackHandler(enabled = trail.isNotEmpty()) { trail = trail.dropLast(1) }

    viewing?.let { file ->
        SharedViewerScreen(
            session = session,
            file = file,
            siblings = entries.orEmpty(),
            onBack = { viewing = null },
            onSessionEnded = model::onSessionEnded,
        )
        return
    }

    val saveTo = rememberLauncherForActivityResult(SaveSharedDocument()) { destination ->
        val file = pendingDownload
        pendingDownload = null
        if (file != null && destination != null) {
            downloading = true
            scope.launch {
                saveSharedFile(context, session, file, destination)
                downloading = false
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        trail.lastOrNull()?.name ?: SHARED_WITH_ME,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = {
                    if (trail.isNotEmpty()) {
                        IconButton(onClick = { trail = trail.dropLast(1) }) {
                            Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back")
                        }
                    } else {
                        Box(modifier = Modifier.padding(start = 12.dp)) {
                            Icon(painterResource(R.drawable.ic_folder_shared), contentDescription = null)
                        }
                    }
                },
                actions = {
                    IconButton(onClick = { reloads++ }) {
                        Icon(painterResource(R.drawable.ic_refresh), contentDescription = "Refresh")
                    }
                },
            )
        },
        bottomBar = navigationBar,
    ) { padding ->
        LazyColumn(
            modifier = Modifier.padding(padding),
            contentPadding = PaddingValues(bottom = 16.dp),
        ) {
            if (downloading) {
                item { LinearProgressIndicator(modifier = Modifier.fillMaxWidth()) }
            }
            if (trail.isNotEmpty()) {
                item {
                    Crumbs(trail, onOpen = { depth -> trail = trail.take(depth) })
                }
            }
            error?.let { problem ->
                item {
                    Text(
                        problem,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
            }
            val shown = entries
            if (shown != null && shown.isEmpty() && error == null) {
                item {
                    ListItem(
                        headlineContent = { Text(if (trail.isEmpty()) "Nothing shared" else EMPTY_FOLDER) },
                        supportingContent = if (trail.isEmpty()) {
                            { Text(NOTHING_SHARED) }
                        } else {
                            null
                        },
                    )
                }
            }
            items(shown.orEmpty(), key = { "${it.shareId}-${it.id}" }) { entry ->
                SharedRow(
                    entry = entry,
                    atTop = trail.isEmpty(),
                    onOpen = {
                        when {
                            entry.kind == EntryKind.FOLDER -> trail = trail + entry
                            entry.hasViewer -> viewing = entry
                            else -> acting = entry
                        }
                    },
                    onMore = { acting = entry },
                )
            }
        }
    }

    acting?.let { entry ->
        SharedActions(
            entry = entry,
            onDismiss = { acting = null },
            onOpen = when {
                entry.kind == EntryKind.FOLDER -> {
                    {
                        acting = null
                        trail = trail + entry
                    }
                }
                entry.hasViewer -> {
                    {
                        acting = null
                        viewing = entry
                    }
                }
                else -> null
            },
            onDownload = if (entry.kind == EntryKind.FILE) {
                {
                    acting = null
                    pendingDownload = entry
                    saveTo.launch(entry)
                }
            } else {
                null
            },
            onSave = {
                acting = null
                saving = entry
            },
        )
    }
    saving?.let { entry ->
        SaveToFolderDialog(
            summary = "“${entry.name}” · Shared by ${entry.owner}",
            client = session.client,
            save = { folder, _ -> session.client.copyShared(listOf(entry), folder) },
            onDismiss = { saving = null },
            onSaved = { folder ->
                saving = null
                Toast.makeText(context, "Saved a copy of “${entry.name}” to $folder", Toast.LENGTH_LONG).show()
                model.reload()
            },
            onSessionEnded = model::onSessionEnded,
        )
    }
}

/** Whether a built-in viewer shows this file. */
private val SharedEntry.hasViewer get() = kind == EntryKind.FILE && viewerFor(mime, size) != null

@Composable
private fun SharedRow(entry: SharedEntry, atTop: Boolean, onOpen: () -> Unit, onMore: () -> Unit) {
    val supporting = if (atTop) {
        listOfNotNull(
            entry.owner,
            VIEW_ONLY,
            entry.expiresAt?.let { "until ${localDate(it)}" },
        ).joinToString(SEPARATOR)
    } else {
        listOf(entry.sizeLabel, localDate(entry.updatedAt)).filter { it.isNotEmpty() }.joinToString(SEPARATOR)
    }
    ListItem(
        modifier = Modifier.clickable(onClick = onOpen),
        leadingContent = { NodeTile(entry.icon) },
        headlineContent = { Text(entry.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = { Text(supporting, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        trailingContent = {
            IconButton(onClick = onMore) {
                Icon(painterResource(R.drawable.ic_more_vert), contentDescription = "More for ${entry.name}")
            }
        },
    )
}

/** "Shared with me › Holiday › Day 1": each but the last goes back to it. */
@Composable
private fun Crumbs(trail: List<SharedEntry>, onOpen: (depth: Int) -> Unit) {
    val names = listOf(SHARED_WITH_ME) + trail.map { it.name }
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        names.forEachIndexed { depth, name ->
            val last = depth == names.lastIndex
            if (depth > 0) Chevron()
            Text(
                name,
                style = if (last) MaterialTheme.typography.labelLarge else MaterialTheme.typography.bodyMedium,
                color = if (last) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .weight(1f, fill = false)
                    .then(if (last) Modifier else Modifier.clickable { onOpen(depth) }),
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SharedActions(
    entry: SharedEntry,
    onDismiss: () -> Unit,
    onOpen: (() -> Unit)?,
    onDownload: (() -> Unit)?,
    onSave: () -> Unit,
) {
    // Fully open, because a half-open sheet runs under the navigation bar.
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(modifier = Modifier.navigationBarsPadding()) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                NodeTile(entry.icon)
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        entry.name,
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        listOf(entry.sizeLabel, "Shared by ${entry.owner}").filter { it.isNotEmpty() }
                            .joinToString(SEPARATOR),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            HorizontalDivider()
            val plain = MaterialTheme.colorScheme.onSurface
            val openIcon = if (entry.kind == EntryKind.FOLDER) R.drawable.ic_folder_open else R.drawable.ic_visibility
            onOpen?.let { ActionItem("Open", openIcon, plain, onClick = it) }
            onDownload?.let { ActionItem("Save to…", R.drawable.ic_download, plain, "To this phone", it) }
            ActionItem("Save to my files", R.drawable.ic_drive_file_move, plain, "A copy you own", onSave)
        }
    }
}

/** Downloads a shared file where the user chose; the document the system created goes if that fails. */
private suspend fun saveSharedFile(context: Context, session: Session.SignedIn, file: SharedEntry, destination: Uri) {
    val resolver = context.contentResolver
    val temp = File(context.cacheDir, "save/shared-${file.id}")
    var saved = false
    try {
        session.client.saveSharedCopy(file, temp.path, NoDownloadProgress)
        withContext(Dispatchers.IO) {
            val output = resolver.openOutputStream(destination, "wt")
                ?: throw FileNotFoundException(destination.toString())
            output.use { sink -> temp.inputStream().use { it.copyTo(sink) } }
        }
        saved = true
        Toast.makeText(context, "Saved “${file.name}”", Toast.LENGTH_SHORT).show()
    } catch (e: CoreException) {
        Toast.makeText(context, "Could not download “${file.name}”: ${e.fileMessage()}", Toast.LENGTH_LONG).show()
    } catch (e: IOException) {
        Toast.makeText(context, "Could not save “${file.name}”: ${e.message}", Toast.LENGTH_LONG).show()
    } finally {
        withContext(Dispatchers.IO) {
            temp.delete()
            if (!saved) runCatching { DocumentsContract.deleteDocument(resolver, destination) }
        }
    }
}

private object NoDownloadProgress : DownloadListener {
    override fun progress(id: String, done: Long, total: Long) = Unit
}

/** Asks where on the phone a shared file goes. */
private class SaveSharedDocument : ActivityResultContract<SharedEntry, Uri?>() {
    override fun createIntent(context: Context, input: SharedEntry): Intent =
        Intent(Intent.ACTION_CREATE_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType(input.mime.ifBlank { OCTET_STREAM_TYPE })
            .putExtra(Intent.EXTRA_TITLE, input.name)

    override fun parseResult(resultCode: Int, intent: Intent?): Uri? =
        intent?.data.takeIf { resultCode == android.app.Activity.RESULT_OK }
}
