package app.homespace.android

import android.text.format.DateUtils
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.time.Instant
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.MoveMode
import uniffi.homespace_core.RestoreResult
import uniffi.homespace_core.TrashEntry

private val EMPTY_ICON_SIZE = 48.dp

/** Deleting for good, which asks first: some files, or the whole bin. */
private sealed interface TrashAction {
    data class DeleteForever(val entries: List<TrashEntry>) : TrashAction
    data object Empty : TrashAction
}

/** The files the user deleted, to restore to Lost & Found or delete for good. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TrashScreen(
    session: Session.SignedIn,
    model: SessionViewModel,
    onBack: () -> Unit,
    onShowLostAndFound: () -> Unit,
) {
    var entries by remember { mutableStateOf<List<TrashEntry>?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf(emptySet<String>()) }
    var sheetFor by remember { mutableStateOf<TrashEntry?>(null) }
    var confirming by remember { mutableStateOf<TrashAction?>(null) }
    var conflicts by remember { mutableStateOf<Pair<List<TrashEntry>, RestoreResult>?>(null) }
    var working by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var menu by remember { mutableStateOf(false) }
    var reload by remember { mutableStateOf(0) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val selecting = selected.isNotEmpty()

    LaunchedEffect(session.client, model.trashKey, reload) {
        try {
            val found = session.client.trash()
            entries = found
            loadError = null
            // A file that is gone can't stay selected.
            selected = selected intersect found.map { it.id }.toSet()
        } catch (_: CoreException.Unauthorized) {
            model.onSessionEnded()
        } catch (e: CoreException) {
            loadError = e.fileMessage()
        }
    }

    fun restore(chosen: List<TrashEntry>, mode: MoveMode) {
        working = true
        actionError = null
        scope.launch {
            try {
                val result = session.client.restoreTrashed(chosen.map { it.id }, mode)
                working = false
                if (result.conflicts.isNotEmpty()) {
                    conflicts = chosen to result
                    return@launch
                }
                conflicts = null
                selected = emptySet()
                reload++
                if (result.message.isNotEmpty()) {
                    val answer = snackbar.showSnackbar(result.message, actionLabel = "Show", withDismissAction = true)
                    if (answer == SnackbarResult.ActionPerformed) onShowLostAndFound()
                }
            } catch (_: CoreException.Unauthorized) {
                model.onSessionEnded()
            } catch (e: CoreException) {
                working = false
                if (conflicts != null) actionError = e.fileMessage() else snackbar.showSnackbar(e.fileMessage())
            }
        }
    }

    BackHandler(enabled = selecting) { selected = emptySet() }
    BackHandler(enabled = !selecting, onBack = onBack)

    val list = entries.orEmpty()
    Scaffold(
        topBar = {
            if (selecting) {
                TopAppBar(
                    title = { Text("${selected.size} selected") },
                    navigationIcon = {
                        IconButton(onClick = { selected = emptySet() }) {
                            Icon(painterResource(R.drawable.ic_close), contentDescription = "Clear selection")
                        }
                    },
                    actions = {
                        if (selected.size < list.size) {
                            IconButton(onClick = { selected = list.map { it.id }.toSet() }) {
                                Icon(painterResource(R.drawable.ic_select_all), contentDescription = "Select all")
                            }
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.surfaceContainer,
                    ),
                )
            } else {
                TopAppBar(
                    title = { Text("Recycle bin") },
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back")
                        }
                    },
                    actions = {
                        Box {
                            IconButton(onClick = { menu = true }) {
                                Icon(painterResource(R.drawable.ic_more_vert), contentDescription = "More options")
                            }
                            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                                DropdownMenuItem(
                                    text = { Text("Empty recycle bin") },
                                    leadingIcon = {
                                        Icon(painterResource(R.drawable.ic_delete_forever), contentDescription = null)
                                    },
                                    enabled = list.isNotEmpty(),
                                    onClick = {
                                        menu = false
                                        confirming = TrashAction.Empty
                                    },
                                )
                            }
                        }
                    },
                )
            }
        },
        bottomBar = {
            if (selecting) {
                val chosen = list.filter { it.id in selected }
                Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                    Row(modifier = Modifier.navigationBarsPadding().fillMaxWidth().height(TOOLBAR_HEIGHT)) {
                        val item = Modifier.weight(1f).fillMaxHeight()
                        ToolbarItem("Restore", R.drawable.ic_restore_from_trash, enabled = !working, item) {
                            restore(chosen, MoveMode.ABORT)
                        }
                        ToolbarItem("Delete forever", R.drawable.ic_delete_forever, enabled = !working, item) {
                            confirming = TrashAction.DeleteForever(chosen)
                        }
                    }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        val current = entries
        when {
            current == null && loadError == null -> Box(
                modifier = Modifier.padding(padding).fillMaxSize(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }
            current == null -> Column(
                modifier = Modifier.padding(padding).fillMaxSize().padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Text(loadError.orEmpty(), textAlign = TextAlign.Center)
                TextButton(onClick = { reload++ }) { Text("Try again") }
            }
            current.isEmpty() -> EmptyTrash(Modifier.padding(padding))
            else -> LazyColumn(modifier = Modifier.padding(padding).fillMaxSize()) {
                item {
                    Text(
                        "Restored files go to Lost & Found.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
                items(current, key = { it.id }) { entry ->
                    TrashRow(
                        entry = entry,
                        selecting = selecting,
                        isSelected = entry.id in selected,
                        onClick = {
                            if (selecting) {
                                selected = if (entry.id in selected) selected - entry.id else selected + entry.id
                            } else {
                                sheetFor = entry
                            }
                        },
                        onLongClick = { selected = selected + entry.id },
                    )
                }
            }
        }
    }

    sheetFor?.let { entry ->
        ModalBottomSheet(onDismissRequest = { sheetFor = null }) {
            Text(
                entry.name,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            )
            ListItem(
                leadingContent = { Icon(painterResource(R.drawable.ic_restore_from_trash), contentDescription = null) },
                headlineContent = { Text("Restore") },
                supportingContent = { Text("To Lost & Found") },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.clickable(onClick = {
                    sheetFor = null
                    restore(listOf(entry), MoveMode.ABORT)
                }),
            )
            val error = MaterialTheme.colorScheme.error
            ListItem(
                leadingContent = {
                    Icon(painterResource(R.drawable.ic_delete_forever), contentDescription = null, tint = error)
                },
                headlineContent = { Text("Delete forever", color = error) },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.clickable(onClick = {
                    sheetFor = null
                    confirming = TrashAction.DeleteForever(listOf(entry))
                }),
            )
            Spacer(modifier = Modifier.height(16.dp))
        }
    }

    conflicts?.let { (chosen, found) ->
        ConflictsDialog(
            title = found.conflictsTitle,
            conflicts = found.conflicts,
            saving = working,
            error = actionError,
            onCancel = {
                conflicts = null
                actionError = null
            },
            onResolve = { mode -> restore(chosen, mode) },
        )
    }

    confirming?.let { action ->
        val count = when (action) {
            is TrashAction.DeleteForever -> action.entries.size
            TrashAction.Empty -> list.size
        }
        AlertDialog(
            onDismissRequest = { if (!working) confirming = null },
            icon = { Icon(painterResource(R.drawable.ic_delete_forever), contentDescription = null) },
            title = {
                val single = (action as? TrashAction.DeleteForever)?.entries?.singleOrNull()
                Text(single?.let { "Permanently delete “${it.name}”?" } ?: "Permanently delete $count files?")
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("This can't be undone.")
                    actionError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {
                TextButton(
                    enabled = !working,
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    onClick = {
                        working = true
                        actionError = null
                        scope.launch {
                            try {
                                when (action) {
                                    is TrashAction.DeleteForever ->
                                        session.client.deleteForever(action.entries.map { it.id })
                                    TrashAction.Empty -> session.client.emptyTrash()
                                }
                                confirming = null
                                selected = emptySet()
                            } catch (_: CoreException.Unauthorized) {
                                model.onSessionEnded()
                            } catch (e: CoreException) {
                                actionError = e.fileMessage()
                            }
                            working = false
                            reload++
                        }
                    },
                ) { Text(if (working) WORKING else "Delete") }
            },
            dismissButton = {
                TextButton(enabled = !working, onClick = {
                    confirming = null
                    actionError = null
                }) { Text("Cancel") }
            },
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TrashRow(
    entry: TrashEntry,
    selecting: Boolean,
    isSelected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(if (isSelected) colors.secondaryContainer else Color.Transparent)
            .combinedClickable(onLongClick = onLongClick, onClick = onClick)
            .heightIn(min = ROW_MIN_HEIGHT)
            .padding(start = if (selecting) 4.dp else 16.dp, end = 16.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (selecting) {
            // The whole row takes the tap, so the box only shows the state.
            Box(modifier = Modifier.size(MIN_TOUCH_TARGET), contentAlignment = Alignment.Center) {
                Checkbox(checked = isSelected, onCheckedChange = null)
            }
            Spacer(modifier = Modifier.width(8.dp))
        }
        NodeTile(entry.icon)
        Spacer(modifier = Modifier.width(16.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(
                entry.name,
                style = MaterialTheme.typography.bodyLarge,
                color = if (isSelected) colors.onSecondaryContainer else colors.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "${entry.folder} · ${deletedOn(entry.deletedAt)} · ${entry.sizeLabel}",
                style = MaterialTheme.typography.bodyMedium.tabular(),
                color = if (isSelected) colors.onSecondaryContainer else colors.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun EmptyTrash(modifier: Modifier) {
    Column(
        modifier = modifier.fillMaxSize().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(
            painterResource(R.drawable.ic_delete),
            contentDescription = null,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(EMPTY_ICON_SIZE),
        )
        Spacer(modifier = Modifier.height(16.dp))
        Text("The recycle bin is empty", style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        Spacer(modifier = Modifier.height(8.dp))
        Text(
            "Deleted files stay here until you delete them forever.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
    }
}

/** A date in the device's locale, without the time, such as `14 Nov 2026`. */
@Composable
private fun deletedOn(time: Instant): String = DateUtils.formatDateTime(
    LocalContext.current,
    time.toEpochMilli(),
    DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_YEAR or DateUtils.FORMAT_ABBREV_MONTH,
)
