package app.homespace.android

import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.homespace_core.ConflictSide
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry
import uniffi.homespace_core.EntryDetails
import uniffi.homespace_core.EntryIcon
import uniffi.homespace_core.EntryKind
import uniffi.homespace_core.MoveMode
import uniffi.homespace_core.MoveResult
import uniffi.homespace_core.alreadyIn
import uniffi.homespace_core.sendSummary

private val CONFLICTS_HEIGHT = 240.dp
private val DETAILS_TILE_SIZE = 72.dp
private val PICKER_ROW_HEIGHT = 56.dp
private const val WORKING = "Working…"
private const val HOME = "Home"
/** How many names a summary of shared files spells out. */
private const val SUMMARY_NAMES = 3

/** The dialog the browser is showing. */
sealed interface FileAction {
    data class Details(val entry: Entry) : FileAction
    data class Rename(val entry: Entry) : FileAction
    data class Move(val entries: List<Entry>) : FileAction
    data class Delete(val entries: List<Entry>) : FileAction
    /** A new folder in the folder `parentId`, or Home when null. */
    data class NewFolder(val parentId: String?) : FileAction
    /** Files and folders of the server, to send to a device. */
    data class Send(val entries: List<Entry>) : FileAction
    /** Files picked on this phone, to send to a device. */
    data class SendLocal(val files: List<Uri>) : FileAction
}

/** Shows the dialog of `action`. `onDone` means something changed, so the folder is out of date. */
@Composable
internal fun FileDialog(
    action: FileAction,
    client: CoreClient,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
    /** What the details sheet's buttons ask for. */
    onRowAction: (Entry, RowAction) -> Unit,
) {
    when (action) {
        is FileAction.Details -> DetailsSheet(action.entry, client, onDismiss, onSessionEnded, onRowAction)
        is FileAction.Rename -> NameDialog(
            title = "Rename “${action.entry.name}”",
            confirm = "Rename",
            initial = action.entry.name,
            save = { client.rename(action.entry.id, it) },
            onDismiss, onDone, onSessionEnded,
            selectExtension = action.entry.kind != EntryKind.FILE,
        )
        is FileAction.NewFolder -> NameDialog(
            title = "New folder",
            confirm = "Create",
            initial = "",
            save = { client.createFolder(action.parentId, it) },
            onDismiss, onDone, onSessionEnded,
        )
        is FileAction.Move -> MoveDialog(action.entries, client, onDismiss, onDone, onSessionEnded)
        is FileAction.Delete -> DeleteDialog(action.entries, client, onDismiss, onDone, onSessionEnded)
        is FileAction.Send -> {
            val context = LocalContext.current
            SendToDeviceDialog(
                summary = action.entries.joinToString(", ") { it.name },
                client = client,
                send = { target ->
                    client.sendEntries(action.entries, target.userId, target.deviceId)
                    TransferCenter.changed()
                },
                onDismiss = onDismiss,
                onSent = { target ->
                    Toast.makeText(context, "Sent to ${target.recipient}", Toast.LENGTH_LONG).show()
                    onDone()
                },
                onSessionEnded = onSessionEnded,
            )
        }
        is FileAction.SendLocal -> SendLocalDialog(action.files, client, onDismiss, onSessionEnded)
    }
}

/** Sends files picked on this phone, which go up in the background once a device is chosen. */
@Composable
private fun SendLocalDialog(
    files: List<Uri>,
    client: CoreClient,
    onDismiss: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    val context = LocalContext.current
    val shared by produceState<SharedFiles?>(null, files) {
        value = withContext(Dispatchers.IO) { describeShared(context, files) }
    }
    val described = shared ?: return
    SendToDeviceDialog(
        summary = sharedSummary(described),
        client = client,
        send = { target ->
            val prepared = withContext(Dispatchers.IO) { prepareSend(context, described) }
            TransferCenter.send(context, client, prepared, target)
        },
        onDismiss = onDismiss,
        onSent = { target ->
            Toast.makeText(context, "Sending to ${target.recipient}", Toast.LENGTH_LONG).show()
            onDismiss()
        },
        onSessionEnded = onSessionEnded,
    )
}

/**
 * What was picked or shared, counted and named: `3 files · 9.4 MB · a.jpg, b.jpg, c.jpg`, or for
 * shared text its first line: `Link · https://youtu.be/…`.
 */
internal fun sharedSummary(shared: SharedFiles): String {
    // Text is shown as what it says, not as the file it goes in.
    shared.text?.let { text ->
        val kind = when (val links = shared.links?.size) {
            null -> "Text"
            1 -> "Link"
            else -> "$links links"
        }
        return "$kind · ${text.trim().lineSequence().first()}"
    }
    val names = shared.names.take(SUMMARY_NAMES).joinToString(", ")
    val more = (shared.names.size - SUMMARY_NAMES).takeIf { it > 0 }?.let { " and $it more" }.orEmpty()
    val counted = sendSummary(shared.names.size.toUInt(), shared.bytes.toULong())
    // Not every app that shares a file says how big it is.
    val what = if (shared.bytes > 0) counted else counted.substringBefore(" · ")
    return "$what · $names$more"
}

/**
 * Chooses the folder on the server that something is saved into. `save` gets the folder's id, null
 * for Home, and its name.
 */
@Composable
internal fun SaveToFolderDialog(
    summary: String,
    client: CoreClient,
    save: suspend (folder: String?, name: String) -> Unit,
    onDismiss: () -> Unit,
    onSaved: (name: String) -> Unit,
    onSessionEnded: () -> Unit,
) {
    var trail by remember { mutableStateOf(emptyList<Crumb>()) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val folder = trail.lastOrNull()?.name ?: HOME
    FolderPickerDialog(
        title = "Save to",
        summary = summary,
        trail = trail,
        onTrailChange = { trail = it },
        client = client,
        hidden = emptyList(),
        confirm = if (saving) "Saving…" else "Save to $folder",
        canConfirm = !saving,
        error = error,
        onConfirm = {
            saving = true
            error = null
            scope.launch {
                try {
                    save(trail.lastOrNull()?.id, folder)
                    onSaved(folder)
                } catch (_: CoreException.Unauthorized) {
                    onSessionEnded()
                } catch (e: CoreException) {
                    error = e.fileMessage()
                    saving = false
                }
            }
        },
        onDismiss = onDismiss,
        onSessionEnded = onSessionEnded,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DetailsSheet(
    entry: Entry,
    client: CoreClient,
    onDismiss: () -> Unit,
    onSessionEnded: () -> Unit,
    onRowAction: (Entry, RowAction) -> Unit,
) {
    var details by remember { mutableStateOf<EntryDetails?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(entry.id) {
        try {
            details = client.details(entry.id)
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
        } catch (e: CoreException) {
            error = e.fileMessage()
        }
    }
    val choose = { action: RowAction ->
        onDismiss()
        onRowAction(entry, action)
    }
    // Fully open, because a half-open sheet runs under the navigation bar.
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(modifier = Modifier.fillMaxWidth().navigationBarsPadding().verticalScroll(rememberScrollState())) {
            Column(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                NodeTile(entry.icon, size = DETAILS_TILE_SIZE)
                Text(
                    entry.name,
                    style = MaterialTheme.typography.titleLarge,
                    textAlign = TextAlign.Center,
                )
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    OfflineChip(entry.offlineState, "Available offline")
                    if (entry.kind == EntryKind.FILE) StatusChip("Version ${entry.version}", ChipTone.NEUTRAL)
                }
                if (entry.kind == EntryKind.FILE) {
                    Row(
                        modifier = Modifier.padding(top = 4.dp, bottom = 16.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        FilledTonalButton(onClick = { choose(RowAction.OPEN) }) {
                            SheetButtonIcon(R.drawable.ic_open_in_new)
                            Text("Open")
                        }
                        OutlinedButton(onClick = { choose(RowAction.SAVE) }) {
                            SheetButtonIcon(R.drawable.ic_download)
                            Text("Save to…")
                        }
                    }
                }
            }
            HorizontalDivider()
            Column(
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            ) {
                error?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(vertical = 8.dp))
                }
                details?.let { loaded ->
                    DetailRow(R.drawable.ic_category, "Type", loaded.typeLabel)
                    if (loaded.entry.kind == EntryKind.FILE) {
                        DetailRow(R.drawable.ic_straighten, "Size", loaded.entry.sizeLabel)
                    }
                    DetailRow(R.drawable.ic_calendar_add_on, "Created", localDate(loaded.createdAt))
                    DetailRow(R.drawable.ic_edit_calendar, "Modified", localDate(loaded.entry.updatedAt))
                    loaded.deviceLabel?.let { DetailRow(R.drawable.ic_laptop, loaded.deviceTitle, it) }
                    loaded.sourceDevice?.let { DetailRow(R.drawable.ic_smartphone, "Uploaded from", it) }
                }
            }
        }
    }
}

@Composable
private fun SheetButtonIcon(icon: Int) {
    Icon(
        painterResource(icon),
        contentDescription = null,
        modifier = Modifier.padding(end = 8.dp).size(ButtonDefaults.IconSize),
    )
}

@Composable
private fun DetailRow(icon: Int, label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = PICKER_ROW_HEIGHT),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            painterResource(icon),
            contentDescription = null,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Column {
            Text(
                label,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(value, style = MaterialTheme.typography.bodyLarge.tabular())
        }
    }
}

@Composable
private fun NameDialog(
    title: String,
    confirm: String,
    initial: String,
    save: suspend (String) -> Unit,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
    /** Off for a file, whose extension should survive typing over the selection. */
    selectExtension: Boolean = true,
) {
    var name by remember {
        val dot = initial.lastIndexOf('.')
        val end = if (!selectExtension && dot > 0) dot else initial.length
        mutableStateOf(TextFieldValue(initial, TextRange(0, end)))
    }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val submit = submit@{
        val typed = name.text.trim()
        if (typed.isEmpty() || typed == initial) {
            onDismiss()
            return@submit
        }
        saving = true
        error = null
        scope.launch {
            try {
                save(typed)
                onDone()
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
                saving = false
            }
        }
        Unit
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                singleLine = true,
                label = { Text("Name") },
                isError = error != null,
                supportingText = error?.let { message -> { Text(message) } },
                trailingIcon = {
                    if (name.text.isNotEmpty()) {
                        IconButton(onClick = { name = TextFieldValue("") }) {
                            Icon(painterResource(R.drawable.ic_cancel), contentDescription = "Clear name")
                        }
                    }
                },
                modifier = Modifier.focusRequester(focus),
            )
        },
        confirmButton = {
            TextButton(onClick = submit, enabled = !saving) {
                Text(if (saving) WORKING else confirm)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun DeleteDialog(
    entries: List<Entry>,
    client: CoreClient,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val title = entries.singleOrNull()?.let { "Move “${it.name}” to recycle bin?" }
        ?: "Move ${entries.size} items to recycle bin?"
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(painterResource(R.drawable.ic_delete), contentDescription = null) },
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    when {
                        entries.any { it.kind == EntryKind.FOLDER } ->
                            "Files inside folders are moved too. You can restore them from the recycle bin."
                        entries.size == 1 -> "You can restore it from the recycle bin."
                        else -> "You can restore them from the recycle bin."
                    },
                )
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            TextButton(
                enabled = !saving,
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                onClick = {
                    saving = true
                    error = null
                    scope.launch {
                        try {
                            client.delete(entries.map { it.id })
                            onDone()
                        } catch (_: CoreException.Unauthorized) {
                            onSessionEnded()
                        } catch (e: CoreException) {
                            error = e.fileMessage()
                            saving = false
                        }
                    }
                },
            ) { Text(if (saving) WORKING else "Move to bin") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun MoveDialog(
    entries: List<Entry>,
    client: CoreClient,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    // The folders opened on the way down from Home; empty at Home.
    var trail by remember { mutableStateOf(emptyList<Crumb>()) }
    var conflicts by remember { mutableStateOf<MoveResult?>(null) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val moving = remember(entries) { entries.map { it.id } }
    val destination = trail.lastOrNull()?.id

    val move = { mode: MoveMode ->
        saving = true
        error = null
        scope.launch {
            try {
                val result = client.moveNodes(moving, destination, mode)
                if (result.conflicts.isEmpty()) {
                    onDone()
                } else {
                    conflicts = result
                    saving = false
                }
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
                saving = false
            }
        }
        Unit
    }

    conflicts?.let { found ->
        ConflictsDialog(
            found = found,
            saving = saving,
            error = error,
            onCancel = {
                conflicts = null
                error = null
            },
            onResolve = move,
        )
        return
    }

    FolderPickerDialog(
        title = entries.singleOrNull()?.let { "Move “${it.name}”" } ?: "Move ${entries.size} items",
        summary = entries.joinToString(", ") { it.name },
        trail = trail,
        onTrailChange = { trail = it },
        client = client,
        hidden = moving,
        confirm = if (saving) WORKING else "Move to ${trail.lastOrNull()?.name ?: HOME}",
        canConfirm = !saving && !alreadyIn(entries, destination),
        error = error,
        onConfirm = { move(MoveMode.ABORT) },
        onDismiss = onDismiss,
        onSessionEnded = onSessionEnded,
    )
}

/** Chooses a folder on the server; `folder` is `null` for Home. */
@Composable
fun ServerFolderDialog(
    title: String,
    confirm: String,
    client: CoreClient,
    askUploadExisting: Boolean,
    onDismiss: () -> Unit,
    onSessionEnded: () -> Unit,
    onChoose: (folder: String?, label: String, uploadExisting: Boolean) -> Unit,
) {
    var trail by remember { mutableStateOf(emptyList<Crumb>()) }
    // Off, so that adding a full camera roll sends nothing by surprise.
    var uploadExisting by remember { mutableStateOf(false) }
    FolderPickerDialog(
        title = title,
        summary = null,
        trail = trail,
        onTrailChange = { trail = it },
        client = client,
        hidden = emptyList(),
        confirm = confirm,
        canConfirm = true,
        error = null,
        onConfirm = { onChoose(trail.lastOrNull()?.id, trail.lastOrNull()?.name ?: HOME, uploadExisting) },
        onDismiss = onDismiss,
        onSessionEnded = onSessionEnded,
        footer = {
            if (askUploadExisting) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .toggleable(
                            value = uploadExisting,
                            role = Role.Switch,
                            onValueChange = { uploadExisting = it },
                        )
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Upload existing files")
                    Switch(checked = uploadExisting, onCheckedChange = null)
                }
            }
        },
    )
}

/** A full-screen dialog to pick a destination, going down from Home. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FolderPickerDialog(
    title: String,
    /** What is being moved, if anything. */
    summary: String?,
    trail: List<Crumb>,
    onTrailChange: (List<Crumb>) -> Unit,
    client: CoreClient,
    /** The ids of folders that can't be chosen, because they are what is being moved. */
    hidden: List<String>,
    confirm: String,
    canConfirm: Boolean,
    error: String?,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
    onSessionEnded: () -> Unit,
    footer: @Composable () -> Unit = {},
) {
    var folders by remember { mutableStateOf<List<Entry>?>(null) }
    var listError by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var refresh by remember { mutableIntStateOf(0) }
    val destination = trail.lastOrNull()?.id

    LaunchedEffect(destination, refresh) {
        folders = null
        listError = null
        try {
            folders = client.list(destination)
                .filter { it.kind == EntryKind.FOLDER && it.id !in hidden }
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
        } catch (e: CoreException) {
            listError = e.fileMessage()
        }
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    navigationIcon = {
                        IconButton(onClick = onDismiss) {
                            Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                        }
                    },
                )
            },
            bottomBar = {
                Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                    Column(modifier = Modifier.navigationBarsPadding()) {
                        footer()
                        Row(
                            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            TextButton(onClick = { creating = true }) {
                                Icon(
                                    painterResource(R.drawable.ic_create_new_folder),
                                    contentDescription = null,
                                    modifier = Modifier.padding(end = 8.dp).size(ButtonDefaults.IconSize),
                                )
                                Text("New folder", maxLines = 1)
                            }
                            Box(modifier = Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
                                Button(onClick = onConfirm, enabled = canConfirm) {
                                    Text(confirm, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                            }
                        }
                    }
                }
            },
        ) { padding ->
            Column(modifier = Modifier.padding(padding).fillMaxSize()) {
                summary?.let {
                    Card(
                        colors = CardDefaults.cardColors(
                            containerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                        ),
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                    ) {
                        Row(
                            modifier = Modifier.padding(16.dp),
                            horizontalArrangement = Arrangement.spacedBy(16.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Icon(
                                painterResource(R.drawable.ic_drive_file_move),
                                contentDescription = null,
                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Text(
                                it,
                                style = MaterialTheme.typography.bodyMedium,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
                Breadcrumbs(
                    names = listOf(HOME) + trail.map { it.name },
                    onOpen = { index -> onTrailChange(trail.take(index)) },
                    modifier = Modifier.padding(vertical = 4.dp),
                )
                HorizontalDivider()
                (error ?: listError)?.let {
                    Text(
                        it,
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
                LazyColumn(modifier = Modifier.weight(1f)) {
                    if (trail.isNotEmpty()) {
                        item(key = "up") {
                            val parent = trail.getOrNull(trail.lastIndex - 1)?.name ?: HOME
                            PickerRow(onClick = { onTrailChange(trail.dropLast(1)) }) {
                                LeadingIcon(R.drawable.ic_arrow_upward)
                                Text("Up to $parent", style = MaterialTheme.typography.bodyLarge)
                            }
                        }
                    }
                    items(folders.orEmpty(), key = { it.id }) { folder ->
                        PickerRow(onClick = { onTrailChange(trail + Crumb(folder.id, folder.name)) }) {
                            NodeTile(EntryIcon.FOLDER)
                            Text(
                                folder.name,
                                style = MaterialTheme.typography.bodyLarge,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f),
                            )
                            Chevron()
                        }
                    }
                    if (folders?.isEmpty() == true) {
                        item(key = "empty") {
                            Text(
                                "No subfolders",
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(16.dp),
                            )
                        }
                    }
                }
            }
        }
        if (creating) {
            NameDialog(
                title = "New folder",
                confirm = "Create",
                initial = "",
                save = { client.createFolder(destination, it) },
                onDismiss = { creating = false },
                onDone = {
                    creating = false
                    refresh++
                },
                onSessionEnded = onSessionEnded,
            )
        }
    }
}

@Composable
private fun PickerRow(onClick: () -> Unit, content: @Composable () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = PICKER_ROW_HEIGHT)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) { content() }
}

@Composable
private fun describe(side: ConflictSide): String =
    "${localDate(side.updatedAt)} · ${side.sizeLabel}"

/** Files already at the destination; cancelling goes back to choosing another folder. */
@Composable
private fun ConflictsDialog(
    found: MoveResult,
    saving: Boolean,
    error: String?,
    onCancel: () -> Unit,
    onResolve: (MoveMode) -> Unit,
) {
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text(found.conflictsTitle) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Override replaces the current content and keeps it as a version.")
                LazyColumn(
                    modifier = Modifier.heightIn(max = CONFLICTS_HEIGHT),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    items(found.conflicts, key = { it.path }) { conflict ->
                        Column {
                            Text(conflict.name)
                            val small = MaterialTheme.typography.bodySmall
                            Text(conflict.path, style = small)
                            Text("Incoming: ${describe(conflict.incoming)}", style = small)
                            val existing = conflict.existing?.let { describe(it) } ?: "Also selected"
                            Text("Existing: $existing", style = small)
                        }
                    }
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            TextButton(
                enabled = !saving,
                onClick = { onResolve(MoveMode.OVERRIDE) },
            ) { Text(if (saving) WORKING else "Override") }
        },
        dismissButton = {
            Row {
                TextButton(onClick = onCancel) { Text("Cancel") }
                TextButton(
                    enabled = !saving,
                    onClick = { onResolve(MoveMode.SKIP) },
                ) { Text("Skip these files") }
            }
        },
    )
}
