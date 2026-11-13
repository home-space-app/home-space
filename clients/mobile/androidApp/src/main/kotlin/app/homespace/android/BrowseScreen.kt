package app.homespace.android

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.text.format.DateUtils
import android.webkit.MimeTypeMap
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContract
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import java.io.File
import java.time.Instant
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry
import uniffi.homespace_core.EntryIcon
import uniffi.homespace_core.EntryKind
import uniffi.homespace_core.LiveEvent
import uniffi.homespace_core.OfflineState
import uniffi.homespace_core.QueuedUpload
import uniffi.homespace_core.QueuedUploadState
import uniffi.homespace_core.viewerFor

private val BADGE_SIZE = 14.dp
private val BADGE_PADDING = 1.dp
private val BADGE_OVERHANG = 3.dp
private val TOP_BAR_AVATAR_SIZE = 32.dp
private val MIN_TOUCH_TARGET = 48.dp
private val SHEET_TILE_SIZE = 48.dp
private val SHEET_ITEM_HEIGHT = 56.dp
private val TOOLBAR_HEIGHT = 80.dp
// Material's navigation bar, which the FAB menu stands above as the FAB does.
private val NAVIGATION_BAR_HEIGHT = 80.dp
// So that the add button doesn't cover the last row's menu.
private val LIST_END_PADDING = 88.dp
private const val SCRIM_ALPHA = 0.32f
private const val DISABLED_ALPHA = 0.38f
private const val OCTET_STREAM = "application/octet-stream"
private const val ANY_FILE = "*/*"

/** What a row's menu can ask for. */
internal enum class RowAction {
    OPEN, OPEN_WITH, SHOW_IN_FOLDER, SAVE, SEND, EDIT, KEEP_OFFLINE, DETAILS, RENAME, MOVE, DELETE, REMOVE_OFFLINE, CANCEL_DOWNLOAD
}

/** Folder browsing through the Rust client (`clients/mobile-core`). */
@Composable
fun BrowseScreen(
    session: Session.SignedIn,
    model: SessionViewModel,
    navigationBar: @Composable () -> Unit,
    onOpenTransfers: () -> Unit,
    onOpenProfile: () -> Unit,
) {
    val path = model.path
    val entries = remember { mutableStateListOf<Entry>() }
    var error by remember { mutableStateOf<String?>(null) }
    // The ids of the selected rows; a long press starts selecting, and it ends once this is empty.
    var selected by remember { mutableStateOf(emptySet<String>()) }
    val selecting = selected.isNotEmpty()
    var action by remember { mutableStateOf<FileAction?>(null) }
    var searching by rememberSaveable { mutableStateOf(false) }
    var adding by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // The upload goes ahead either way. Without this, the app's progress can't show outside it.
    val askNotifications = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }
    val pickFiles = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenMultipleDocuments(),
    ) { files ->
        val folder = path.lastOrNull()
        if (files.isNotEmpty() && folder != null) {
            model.uploadFiles(files, folder)
            if (needsNotificationPermission(context)) {
                askNotifications.launch(android.Manifest.permission.POST_NOTIFICATIONS)
            }
        }
    }
    val pickToSend = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenMultipleDocuments(),
    ) { files ->
        if (files.isNotEmpty()) action = FileAction.SendLocal(files)
    }
    val saveTo = rememberLauncherForActivityResult(SaveDocument()) { destination ->
        val entry = model.pendingSave
        model.pendingSave = null
        if (entry != null && destination != null) model.saveFile(session, entry, destination)
    }

    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, _ ->
            model.foreground = lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            model.foreground = false
        }
    }

    model.pendingOpen?.let { request ->
        LaunchedEffect(request) {
            if (!openInViewer(context, request)) {
                Toast.makeText(context, "No app can open this file", Toast.LENGTH_LONG).show()
            }
            model.onOpened()
        }
    }

    BackHandler(enabled = path.size > 1) { path.removeAt(path.lastIndex) }
    // Declared last so it goes first: back leaves the selection before it leaves the folder.
    BackHandler(enabled = selecting) { selected = emptySet() }

    LaunchedEffect(path.lastOrNull()) { selected = emptySet() }

    LaunchedEffect(session.client, path.lastOrNull(), model.reloadKey) {
        val folder = path.lastOrNull() ?: return@LaunchedEffect
        error = null
        try {
            val listed = session.client.list(folder.id)
            entries.clear()
            entries.addAll(listed)
            // A file that is gone can't stay selected.
            val present = listed.map { it.id }.toSet()
            selected = selected intersect present
        } catch (_: CoreException.Unauthorized) {
            model.onSessionEnded()
        } catch (e: CoreException) {
            error = e.fileMessage()
        }
    }

    // Changes from other devices: reload when they touch the folder on screen, or when some may
    // have been missed, as after the app was in the background.
    LaunchedEffect(model) {
        model.liveEvents.collect { event ->
            val stale = when (event) {
                is LiveEvent.Resync -> true
                is LiveEvent.NodeChanged ->
                    // The current path, not the one this effect started with.
                    event.parentId == model.path.lastOrNull()?.id ||
                        entries.any { it.id == event.id }
                else -> false
            }
            if (stale) model.reloadSoon()
        }
    }

    // Updating and failed copies change while the folder is open, with nothing new on the server.
    val syncChanges by ClientHolder.syncChanges.collectAsState()
    LaunchedEffect(session.client, syncChanges) {
        if (entries.isEmpty()) return@LaunchedEffect
        val marked = session.client.refreshOffline(entries.toList())
        if (marked.map { it.id } == entries.map { it.id }) {
            marked.forEachIndexed { index, entry -> entries[index] = entry }
        }
    }

    fun handle(entry: Entry, rowAction: RowAction) {
        when (rowAction) {
            RowAction.OPEN -> model.openFile(session, entry)
            RowAction.SHOW_IN_FOLDER -> scope.launch {
                if (model.showInFolder(session, entry)) searching = false
            }
            RowAction.SAVE -> {
                model.pendingSave = entry
                saveTo.launch(entry)
            }
            RowAction.SEND -> action = FileAction.Send(listOf(entry))
            RowAction.OPEN_WITH -> model.openInAnotherApp(session, entry)
            RowAction.EDIT -> model.openFile(session, entry, edit = true)
            RowAction.KEEP_OFFLINE -> model.makeAvailableOffline(session, entry)
            RowAction.DETAILS -> action = FileAction.Details(entry)
            RowAction.RENAME -> action = FileAction.Rename(entry)
            RowAction.MOVE -> action = FileAction.Move(listOf(entry))
            RowAction.DELETE -> action = FileAction.Delete(listOf(entry))
            RowAction.REMOVE_OFFLINE -> model.removeOfflineCopy(session, entry)
            RowAction.CANCEL_DOWNLOAD -> session.client.cancelDownload(entry.id)
        }
    }

    if (searching) {
        SearchScreen(
            session = session,
            model = model,
            onClose = { searching = false },
            onOpenFolder = { folder ->
                scope.launch { if (model.showInFolder(session, folder)) searching = false }
            },
            onAction = ::handle,
        )
    } else {
        Box(modifier = Modifier.fillMaxSize()) {
            val chosen = entries.filter { it.id in selected }
            Scaffold(
                topBar = {
                    if (selecting) {
                        SelectionTopBar(
                            count = selected.size,
                            canSelectMore = selected.size < entries.size,
                            onClose = { selected = emptySet() },
                            onSelectAll = { selected = entries.map { it.id }.toSet() },
                        )
                    } else {
                        FolderTopBar(
                            title = path.lastOrNull()?.name.orEmpty(),
                            username = session.user.username,
                            onBack = if (path.size > 1) ({ path.removeAt(path.lastIndex) }) else null,
                            onSearch = { searching = true },
                            onOpenProfile = onOpenProfile,
                        )
                    }
                },
                bottomBar = {
                    if (selecting) {
                        SelectionToolbar(
                            chosen = chosen,
                            canDelete = chosen.none { it.id == session.user.lostAndFoundId },
                            onAction = { action = it },
                            onKeepOffline = { keeping ->
                                keeping.forEach { model.makeAvailableOffline(session, it) }
                                selected = emptySet()
                            },
                            onRemoveOffline = { removing ->
                                // If several hold unsent edits, one asks and the rest stay kept.
                                removing.forEach { model.removeOfflineCopy(session, it) }
                                selected = emptySet()
                            },
                        )
                    } else {
                        navigationBar()
                    }
                },
                floatingActionButton = {
                    // The menu has its own button in this place.
                    if (!selecting && !adding) {
                        ExtendedFloatingActionButton(
                            onClick = { adding = true },
                            icon = { Icon(painterResource(R.drawable.ic_add), contentDescription = null) },
                            text = { Text("New") },
                            modifier = Modifier.semantics { contentDescription = "New" },
                        )
                    }
                },
            ) { padding ->
                Column(modifier = Modifier.padding(padding).fillMaxSize()) {
                    if (!selecting && path.size > 1) {
                        Breadcrumbs(
                            names = path.map { it.name },
                            onOpen = { index -> path.removeRange(index + 1, path.size) },
                            modifier = Modifier.padding(bottom = 4.dp),
                        )
                    }
                    error?.let {
                        Text(
                            it,
                            color = MaterialTheme.colorScheme.error,
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                        )
                    }
                    // Above the list, not in it: it stays in view however far the list is scrolled.
                    if (!selecting && model.uploads.isNotEmpty()) {
                        UploadsSummaryCard(model.uploads, onOpenTransfers)
                    }
                    LazyColumn(contentPadding = PaddingValues(bottom = LIST_END_PADDING)) {
                        items(entries, key = { it.id }) { entry ->
                            val toggle = {
                                selected = if (entry.id in selected) selected - entry.id else selected + entry.id
                            }
                            EntryRow(
                                entry = entry,
                                selecting = selecting,
                                isSelected = entry.id in selected,
                                progress = model.downloads[entry.id],
                                canDelete = entry.id != session.user.lostAndFoundId,
                                onClick = {
                                    when {
                                        selecting -> toggle()
                                        entry.kind == EntryKind.FOLDER -> path.add(Crumb(entry.id, entry.name))
                                        else -> model.openFile(session, entry)
                                    }
                                },
                                onLongClick = toggle,
                                onAction = { handle(entry, it) },
                            )
                        }
                    }
                }
            }

            AddMenu(
                visible = adding && !selecting,
                onDismiss = { adding = false },
                onNewFolder = {
                    adding = false
                    action = FileAction.NewFolder(path.lastOrNull()?.id)
                },
                onUpload = {
                    adding = false
                    pickFiles.launch(arrayOf(ANY_FILE))
                },
                onSend = {
                    adding = false
                    pickToSend.launch(arrayOf(ANY_FILE))
                },
            )
        }
    }

    action?.let { current ->
        FileDialog(
            action = current,
            client = session.client,
            onDismiss = { action = null },
            onDone = {
                action = null
                selected = emptySet()
                model.reload()
            },
            onSessionEnded = model::onSessionEnded,
            onRowAction = ::handle,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FolderTopBar(
    title: String,
    username: String,
    onBack: (() -> Unit)?,
    onSearch: () -> Unit,
    onOpenProfile: () -> Unit,
) {
    TopAppBar(
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            if (onBack != null) {
                IconButton(onClick = onBack) {
                    Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back")
                }
            }
        },
        actions = {
            IconButton(onClick = onSearch) {
                Icon(painterResource(R.drawable.ic_search), contentDescription = "Search")
            }
            IconButton(
                onClick = onOpenProfile,
                modifier = Modifier.semantics { contentDescription = "Profile" },
            ) {
                Avatar(username, TOP_BAR_AVATAR_SIZE, MaterialTheme.typography.labelLarge)
            }
        },
    )
}

/** The bar while rows are selected. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SelectionTopBar(count: Int, canSelectMore: Boolean, onClose: () -> Unit, onSelectAll: () -> Unit) {
    TopAppBar(
        title = { Text("$count selected") },
        navigationIcon = {
            IconButton(onClick = onClose) {
                Icon(painterResource(R.drawable.ic_close), contentDescription = "Clear selection")
            }
        },
        actions = {
            if (canSelectMore) {
                IconButton(onClick = onSelectAll) {
                    Icon(painterResource(R.drawable.ic_select_all), contentDescription = "Select all")
                }
            }
        },
        colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
    )
}

/** What can be done with the selected rows. */
@Composable
private fun SelectionToolbar(
    chosen: List<Entry>,
    canDelete: Boolean,
    onAction: (FileAction) -> Unit,
    onKeepOffline: (List<Entry>) -> Unit,
    onRemoveOffline: (List<Entry>) -> Unit,
) {
    val single = chosen.singleOrNull()
    val notKept = chosen.filter { !it.offlineKept && it.offlineState == null }
    val kept = chosen.filter { it.offlineKept }
    var more by remember { mutableStateOf(false) }
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Row(modifier = Modifier.navigationBarsPadding().fillMaxWidth().height(TOOLBAR_HEIGHT)) {
            val item = Modifier.weight(1f).fillMaxHeight()
            ToolbarItem("Move", R.drawable.ic_drive_file_move, enabled = true, item) {
                onAction(FileAction.Move(chosen))
            }
            ToolbarItem("Offline", R.drawable.ic_download, enabled = notKept.isNotEmpty(), item) {
                onKeepOffline(notKept)
            }
            ToolbarItem("Delete", R.drawable.ic_delete, enabled = canDelete, item) {
                onAction(FileAction.Delete(chosen))
            }
            Box(modifier = item) {
                ToolbarItem(
                    "More",
                    R.drawable.ic_more_vert,
                    enabled = true,
                    Modifier.fillMaxSize(),
                ) { more = true }
                DropdownMenu(expanded = more, onDismissRequest = { more = false }) {
                    DropdownMenuItem(
                        text = { Text("Send to device…") },
                        leadingIcon = { Icon(painterResource(R.drawable.ic_send), contentDescription = null) },
                        onClick = {
                            more = false
                            onAction(FileAction.Send(chosen))
                        },
                    )
                    single?.let { entry ->
                        DropdownMenuItem(
                            text = { Text("Rename") },
                            leadingIcon = { Icon(painterResource(R.drawable.ic_edit), contentDescription = null) },
                            onClick = {
                                more = false
                                onAction(FileAction.Rename(entry))
                            },
                        )
                        DropdownMenuItem(
                            text = { Text("Details") },
                            leadingIcon = { Icon(painterResource(R.drawable.ic_info), contentDescription = null) },
                            onClick = {
                                more = false
                                onAction(FileAction.Details(entry))
                            },
                        )
                    }
                    if (kept.isNotEmpty()) {
                        DropdownMenuItem(
                            text = { Text(if (kept.size == 1) "Remove offline copy" else "Remove offline copies") },
                            leadingIcon = {
                                Icon(painterResource(R.drawable.ic_download_off), contentDescription = null)
                            },
                            onClick = {
                                more = false
                                onRemoveOffline(kept)
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun ToolbarItem(label: String, icon: Int, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val tint = MaterialTheme.colorScheme.onSurfaceVariant.let { if (enabled) it else it.copy(alpha = DISABLED_ALPHA) }
    Column(
        modifier = modifier.clickable(enabled = enabled, onClick = onClick),
        verticalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = tint)
        Text(label, style = MaterialTheme.typography.labelMedium, color = tint)
    }
}

/** The FAB menu: what the New button adds to the folder on screen, over a scrim. */
@Composable
private fun AddMenu(
    visible: Boolean,
    onDismiss: () -> Unit,
    onNewFolder: () -> Unit,
    onUpload: () -> Unit,
    onSend: () -> Unit,
) {
    BackHandler(enabled = visible, onBack = onDismiss)
    AnimatedVisibility(visible = visible, enter = fadeIn(), exit = fadeOut()) {
        val navigationBars = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.scrim.copy(alpha = SCRIM_ALPHA))
                .clickable(interactionSource = null, indication = null, onClick = onDismiss),
        ) {
            Column(
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .padding(end = 16.dp, bottom = NAVIGATION_BAR_HEIGHT + navigationBars + 16.dp),
                horizontalAlignment = Alignment.End,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                ExtendedFloatingActionButton(
                    onClick = onSend,
                    icon = { Icon(painterResource(R.drawable.ic_send), contentDescription = null) },
                    text = { Text("Send files to a device") },
                    modifier = Modifier.semantics { contentDescription = "Send files to a device" },
                    shape = CircleShape,
                )
                ExtendedFloatingActionButton(
                    onClick = onUpload,
                    icon = { Icon(painterResource(R.drawable.ic_upload_file), contentDescription = null) },
                    text = { Text("Upload files") },
                    modifier = Modifier.semantics { contentDescription = "Upload files" },
                    shape = CircleShape,
                )
                ExtendedFloatingActionButton(
                    onClick = onNewFolder,
                    icon = { Icon(painterResource(R.drawable.ic_create_new_folder), contentDescription = null) },
                    text = { Text("New folder") },
                    modifier = Modifier.semantics { contentDescription = "New folder" },
                    shape = CircleShape,
                )
                FloatingActionButton(
                    onClick = onDismiss,
                    shape = CircleShape,
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                    modifier = Modifier.padding(top = 8.dp),
                ) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                }
            }
        }
    }
}

/** The uploads as one card in every folder, so that a failure can't go unseen. Opens Transfers. */
@Composable
private fun UploadsSummaryCard(uploads: List<QueuedUpload>, onClick: () -> Unit) {
    val failed = uploads.count { it.state == QueuedUploadState.FAILED }
    val going = uploads.filter { it.state != QueuedUploadState.FAILED }
    val total = going.sumOf { it.total.toLong() }
    val progress = if (total > 0) going.sumOf { it.done.toLong() }.toFloat() / total else null
    val title = when {
        going.isEmpty() && failed == 1 -> "1 file couldn't be uploaded"
        going.isEmpty() -> "$failed files couldn't be uploaded"
        going.size == 1 -> "Uploading ${going[0].name}"
        else -> "Uploading ${going.size} files"
    }
    val detail = buildList {
        if (going.any { it.state == QueuedUploadState.OFFLINE }) {
            add("Waiting for the connection")
        } else if (progress != null) {
            add("${(progress * PERCENT).toInt()}%")
        }
        if (going.isNotEmpty() && failed > 0) add("$failed failed")
    }.joinToString(" · ")
    Card(
        onClick = onClick,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHighest),
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    painterResource(R.drawable.ic_upload),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                )
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        title,
                        style = MaterialTheme.typography.titleSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (detail.isNotEmpty()) {
                        Text(
                            detail,
                            style = MaterialTheme.typography.bodySmall.tabular(),
                            color = if (going.isEmpty()) {
                                MaterialTheme.colorScheme.error
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        )
                    }
                }
                Chevron()
            }
            if (going.isNotEmpty()) ProgressLine(progress, modifier = Modifier.fillMaxWidth())
        }
    }
}

/** The name with what was searched for in bold. */
private fun highlighted(name: String, query: String?): AnnotatedString = buildAnnotatedString {
    append(name)
    if (query.isNullOrEmpty()) return@buildAnnotatedString
    var start = name.indexOf(query, ignoreCase = true)
    while (start >= 0) {
        val end = start + query.length
        addStyle(SpanStyle(fontWeight = FontWeight.Bold), start, end)
        start = name.indexOf(query, startIndex = end, ignoreCase = true)
    }
}

@Composable
internal fun EntryRow(
    entry: Entry,
    selecting: Boolean,
    isSelected: Boolean,
    progress: Float?,
    canDelete: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    onAction: (RowAction) -> Unit,
    /** For a search result: where it is, in place of its size and date. */
    location: String? = null,
    /** For a search result: what was searched for. */
    highlight: String? = null,
) {
    val colors = MaterialTheme.colorScheme
    val background = if (isSelected) colors.secondaryContainer else Color.Transparent
    val headline = if (isSelected) colors.onSecondaryContainer else colors.onSurface
    val supporting = if (isSelected) colors.onSecondaryContainer else colors.onSurfaceVariant
    val failed = entry.offlineState == OfflineState.FAILED && progress == null
    val detail = when {
        progress != null -> "Downloading · ${(progress * PERCENT).toInt()}%"
        location != null -> location
        entry.kind != EntryKind.FILE -> null
        failed -> "Sync failed"
        entry.offlineState == OfflineState.UPDATING -> "${entry.sizeLabel} · Syncing…"
        else -> "${entry.sizeLabel} · ${localDate(entry.updatedAt)}"
    }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(background)
            .combinedClickable(onLongClick = onLongClick, onClick = onClick),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = ROW_MIN_HEIGHT)
                .padding(start = if (selecting) 4.dp else 16.dp, end = 4.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (selecting) {
                // The whole row takes the tap, so the box only shows the state.
                Box(modifier = Modifier.size(MIN_TOUCH_TARGET), contentAlignment = Alignment.Center) {
                    Checkbox(checked = isSelected, onCheckedChange = null)
                }
                Spacer(modifier = Modifier.width(8.dp))
            }
            BadgedTile(entry.icon, state = entry.offlineState.takeIf { progress == null })
            Spacer(modifier = Modifier.width(16.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    highlighted(entry.name, highlight),
                    style = MaterialTheme.typography.bodyLarge,
                    color = headline,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                detail?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodyMedium.tabular(),
                        color = if (failed && location == null) colors.error else supporting,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            if (!selecting) {
                if (progress != null && location == null) {
                    IconButton(onClick = { onAction(RowAction.CANCEL_DOWNLOAD) }) {
                        Icon(
                            painterResource(R.drawable.ic_close),
                            contentDescription = "Cancel download of ${entry.name}",
                            tint = colors.onSurfaceVariant,
                        )
                    }
                } else {
                    RowMenu(entry, downloading = progress != null, canDelete, inSearch = location != null, onAction)
                }
            }
        }
        if (progress != null) {
            ProgressLine(
                progress,
                modifier = Modifier.fillMaxWidth().padding(start = ROW_TEXT_START, end = 16.dp, bottom = 8.dp),
            )
        }
    }
}

/** The button of a row's actions, which open in a sheet with the file as its header. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RowMenu(
    entry: Entry,
    downloading: Boolean,
    canDelete: Boolean,
    inSearch: Boolean,
    onAction: (RowAction) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    IconButton(onClick = { expanded = true }) {
        Icon(
            painterResource(R.drawable.ic_more_vert),
            contentDescription = "Actions for ${entry.name}",
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    if (!expanded) return
    val choose = { action: RowAction ->
        expanded = false
        onAction(action)
    }
    // Fully open, because a half-open sheet runs under the navigation bar.
    ModalBottomSheet(
        onDismissRequest = { expanded = false },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(modifier = Modifier.navigationBarsPadding().verticalScroll(rememberScrollState())) {
            ActionsHeader(entry)
            HorizontalDivider()
            val item = @Composable { label: String, icon: Int, action: RowAction ->
                ActionItem(label, icon, MaterialTheme.colorScheme.onSurface) { choose(action) }
            }
            if (inSearch) item("Show in folder", R.drawable.ic_folder, RowAction.SHOW_IN_FOLDER)
            if (downloading) item("Cancel download", R.drawable.ic_close, RowAction.CANCEL_DOWNLOAD)
            if (entry.kind == EntryKind.FILE && !downloading) {
                if (viewerFor(entry.mime, entry.size) != null) {
                    item("Open in another app", R.drawable.ic_open_in_new, RowAction.OPEN_WITH)
                }
                item("Edit in another app", R.drawable.ic_edit_document, RowAction.EDIT)
                item("Save to…", R.drawable.ic_download, RowAction.SAVE)
            }
            if (!downloading) item("Send to device…", R.drawable.ic_send, RowAction.SEND)
            if (entry.offlineKept && !downloading) {
                item("Remove offline copy", R.drawable.ic_download_off, RowAction.REMOVE_OFFLINE)
            } else if (entry.offlineState == null && !downloading) {
                item("Make available offline", R.drawable.ic_download, RowAction.KEEP_OFFLINE)
            }
            item("Details", R.drawable.ic_info, RowAction.DETAILS)
            item("Rename", R.drawable.ic_edit, RowAction.RENAME)
            item("Move", R.drawable.ic_drive_file_move, RowAction.MOVE)
            if (canDelete) {
                HorizontalDivider()
                ActionItem("Move to bin", R.drawable.ic_delete, MaterialTheme.colorScheme.error) {
                    choose(RowAction.DELETE)
                }
            }
        }
    }
}

@Composable
private fun ActionsHeader(entry: Entry) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        NodeTile(entry.icon, size = SHEET_TILE_SIZE)
        Column(modifier = Modifier.weight(1f)) {
            Text(
                entry.name,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (entry.kind == EntryKind.FILE) {
                Text(
                    "${entry.sizeLabel} · ${localDate(entry.updatedAt)}",
                    style = MaterialTheme.typography.bodyMedium.tabular(),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        OfflineChip(entry.offlineState, "Offline")
    }
}

/** A copy's state in words; `kept` says it for a copy that is up to date. */
@Composable
internal fun OfflineChip(state: OfflineState?, kept: String) {
    when (state) {
        null -> Unit
        OfflineState.SYNCED -> StatusChip(kept, ChipTone.SUCCESS, R.drawable.ic_check_circle)
        OfflineState.UPDATING -> StatusChip("Syncing", ChipTone.NEUTRAL)
        OfflineState.FAILED -> StatusChip("Sync failed", ChipTone.ERROR, R.drawable.ic_error_outline)
    }
}

/** An action of a bottom sheet; `hint` says at its end what it leads to. */
@Composable
internal fun ActionItem(label: String, icon: Int, color: Color, hint: String? = null, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = SHEET_ITEM_HEIGHT)
            .padding(horizontal = 24.dp),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            painterResource(icon),
            contentDescription = null,
            tint = if (color == MaterialTheme.colorScheme.error) color else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(label, style = MaterialTheme.typography.bodyLarge, color = color, modifier = Modifier.weight(1f))
        hint?.let {
            Text(
                it,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** A time in the device's locale, time zone and 12 or 24 hour setting. */
@Composable
internal fun localDate(time: Instant): String = DateUtils.formatDateTime(
    LocalContext.current,
    time.toEpochMilli(),
    DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME or
        DateUtils.FORMAT_SHOW_YEAR or DateUtils.FORMAT_ABBREV_MONTH,
)

/** A node's tile, with the desktop client's badge in the corner once it is kept on the phone. */
@Composable
private fun BadgedTile(icon: EntryIcon, state: OfflineState?) {
    Box {
        NodeTile(icon)
        if (state != null) {
            val (drawable, label, tint) = when (state) {
                OfflineState.SYNCED -> Triple(
                    R.drawable.ic_check_circle,
                    "Available offline",
                    LocalExtendedColors.current.success,
                )
                OfflineState.UPDATING -> Triple(R.drawable.ic_sync, "Syncing", MaterialTheme.colorScheme.primary)
                OfflineState.FAILED -> Triple(
                    R.drawable.ic_error,
                    "Sync failed",
                    MaterialTheme.colorScheme.error,
                )
            }
            Box(
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .offset(x = BADGE_OVERHANG, y = BADGE_OVERHANG)
                    .background(MaterialTheme.colorScheme.surface, CircleShape)
                    .padding(BADGE_PADDING),
            ) {
                Icon(
                    painterResource(drawable),
                    contentDescription = label,
                    tint = tint,
                    modifier = Modifier.size(BADGE_SIZE),
                )
            }
        }
    }
}

/** Asks the system where to save a file, offering its name and type. */
private class SaveDocument : ActivityResultContract<Entry, Uri?>() {
    override fun createIntent(context: Context, input: Entry): Intent =
        Intent(Intent.ACTION_CREATE_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType(input.mime.ifBlank { OCTET_STREAM })
            .putExtra(Intent.EXTRA_TITLE, input.name)

    override fun parseResult(resultCode: Int, intent: Intent?): Uri? =
        intent?.data.takeIf { resultCode == android.app.Activity.RESULT_OK }
}

/**
 * Hands the file to whichever app handles its type: read-only to look at it, with write access to
 * edit it, which the sync then picks up. False when no app does.
 */
private fun openInViewer(context: Context, request: OpenRequest): Boolean {
    val file = File(request.path)
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val type = request.mime.takeUnless { it.isBlank() || it == OCTET_STREAM }
        ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase())
        ?: "*/*"
    val intent = Intent(if (request.edit) Intent.ACTION_EDIT else Intent.ACTION_VIEW)
        .setDataAndType(uri, type)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    if (request.edit) intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
    return try {
        context.startActivity(intent)
        true
    } catch (_: ActivityNotFoundException) {
        false
    }
}
