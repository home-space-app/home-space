package app.homespace.android

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.CurrentUpload
import uniffi.homespace_core.OfflineStatus
import uniffi.homespace_core.SourceKind
import uniffi.homespace_core.SyncSettings
import uniffi.homespace_core.UploadSource

private val SETTINGS_AVATAR_SIZE = 40.dp
// So that the add button doesn't cover the last card's buttons.
private val FOLDERS_END_PADDING = 88.dp

/** A local source picked, whose server folder is still to be chosen. */
private data class PickedSource(val kind: SourceKind, val handle: String, val label: String)

/** What the folders screen is asking the user. */
private sealed interface SyncDialog {
    data object AddSource : SyncDialog
    data class ChooseTarget(val source: PickedSource) : SyncDialog
    data class ChangeTarget(val source: UploadSource) : SyncDialog
    data class Remove(val source: UploadSource) : SyncDialog
}

/** The account, the sync settings and how the app looks. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    session: Session.SignedIn,
    model: SessionViewModel,
    navigationBar: @Composable () -> Unit,
    onOpenPage: (SubPage) -> Unit,
) {
    val context = LocalContext.current
    val active = ClientHolder.active ?: return
    val auto = active.autoUpload
    val changes by ClientHolder.syncChanges.collectAsState()
    var settings by remember { mutableStateOf(auto.settings()) }
    var batteryHint by remember { mutableStateOf(false) }
    var choosingTheme by remember { mutableStateOf(false) }
    val sources = remember(changes) { runCatching { auto.sources() }.getOrDefault(emptyList()) }
    val offline by produceState<OfflineStatus?>(null, changes) {
        value = runCatching { session.client.offlineStatus() }.getOrNull()
    }

    fun update(changed: SyncSettings) {
        auto.setSettings(changed)
        settings = changed
        applyBackgroundSetting(context, changed)
        model.syncNow()
    }

    val askNotifications = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) {
        // Uploads run without it too, only with no sign of them. So the answer changes nothing.
        update(settings.copy(background = true))
        batteryHint = true
    }

    Scaffold(
        topBar = { TopAppBar(title = { Text("Settings") }) },
        bottomBar = navigationBar,
    ) { padding ->
        Column(modifier = Modifier.padding(padding).fillMaxSize().verticalScroll(rememberScrollState())) {
            ListItem(
                leadingContent = {
                    Avatar(session.user.username, SETTINGS_AVATAR_SIZE, MaterialTheme.typography.titleMedium)
                },
                headlineContent = { Text(session.user.username, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                supportingContent = {
                    Text(serverLabel(session.client.baseUrl()), maxLines = 1, overflow = TextOverflow.Ellipsis)
                },
                trailingContent = { Chevron() },
                modifier = Modifier.clickable { onOpenPage(SubPage.PROFILE) },
            )
            HorizontalDivider()
            SectionHeader("Files")
            ListItem(
                leadingContent = { LeadingIcon(R.drawable.ic_delete) },
                headlineContent = { Text("Recycle bin") },
                trailingContent = { Chevron() },
                modifier = Modifier.clickable { onOpenPage(SubPage.TRASH) },
            )
            SectionHeader("Sync")
            ListItem(
                leadingContent = { LeadingIcon(R.drawable.ic_wifi) },
                headlineContent = { Text("Sync only on Wi-Fi") },
                supportingContent = { Text("Files don't sync over mobile data.") },
                trailingContent = { Switch(checked = settings.wifiOnly, onCheckedChange = null) },
                modifier = Modifier.toggleable(
                    value = settings.wifiOnly,
                    role = Role.Switch,
                    onValueChange = { update(settings.copy(wifiOnly = it)) },
                ),
            )
            ListItem(
                leadingContent = { LeadingIcon(R.drawable.ic_autorenew) },
                headlineContent = { Text("Sync in background") },
                supportingContent = {
                    Text("Keep syncing when the app isn't open. Android decides when this runs.")
                },
                trailingContent = { Switch(checked = settings.background, onCheckedChange = null) },
                modifier = Modifier.toggleable(
                    value = settings.background,
                    role = Role.Switch,
                    onValueChange = { on ->
                        when {
                            !on -> update(settings.copy(background = false))
                            needsNotificationPermission(context) ->
                                askNotifications.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                            else -> {
                                update(settings.copy(background = true))
                                batteryHint = true
                            }
                        }
                    },
                ),
            )
            ListItem(
                leadingContent = { LeadingIcon(R.drawable.ic_drive_folder_upload) },
                headlineContent = { Text("Folders that upload") },
                supportingContent = {
                    Text(
                        sources.joinToString(", ") { it.label }.ifEmpty { "None yet" },
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                trailingContent = { Chevron() },
                modifier = Modifier.clickable { onOpenPage(SubPage.UPLOAD_FOLDERS) },
            )
            offline?.let { status ->
                ListItem(
                    leadingContent = { LeadingIcon(R.drawable.ic_offline_pin) },
                    headlineContent = { Text("Offline files") },
                    supportingContent = { Text(offlineCounts(status)) },
                    trailingContent = { Chevron() },
                    modifier = Modifier.clickable { onOpenPage(SubPage.OFFLINE_FILES) },
                )
            }
            SectionHeader("Appearance")
            ListItem(
                leadingContent = { LeadingIcon(R.drawable.ic_dark_mode) },
                headlineContent = { Text("Theme") },
                supportingContent = { Text(model.themeMode.label) },
                modifier = Modifier.clickable { choosingTheme = true },
            )
            if (SYSTEM_COLOURS_AVAILABLE) {
                ListItem(
                    leadingContent = { LeadingIcon(R.drawable.ic_palette) },
                    headlineContent = { Text("Use system colours") },
                    supportingContent = { Text("Match your wallpaper") },
                    trailingContent = { Switch(checked = model.useSystemColours, onCheckedChange = null) },
                    modifier = Modifier.toggleable(
                        value = model.useSystemColours,
                        role = Role.Switch,
                        onValueChange = model::changeUseSystemColours,
                    ),
                )
            }
        }
    }

    if (choosingTheme) {
        ThemeDialog(
            current = model.themeMode,
            onChoose = {
                model.changeThemeMode(it)
                choosingTheme = false
            },
            onDismiss = { choosingTheme = false },
        )
    }
    if (batteryHint) {
        AlertDialog(
            onDismissRequest = { batteryHint = false },
            title = { Text("Sync in background") },
            text = {
                Text(
                    "Android may delay background uploads to save battery. For more regular " +
                        "uploads, set Home Space's battery use to Unrestricted.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    batteryHint = false
                    context.startActivity(appBatterySettings(context))
                }) { Text("Open settings") }
            },
            dismissButton = { TextButton(onClick = { batteryHint = false }) { Text("Not now") } },
        )
    }
}

@Composable
private fun ThemeDialog(current: ThemeMode, onChoose: (ThemeMode) -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Theme") },
        text = {
            Column(modifier = Modifier.selectableGroup()) {
                ThemeMode.entries.forEach { mode ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .selectable(
                                selected = mode == current,
                                role = Role.RadioButton,
                                onClick = { onChoose(mode) },
                            )
                            .padding(vertical = 12.dp),
                        horizontalArrangement = Arrangement.spacedBy(16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        // The whole row takes the tap.
                        RadioButton(selected = mode == current, onClick = null)
                        Text(mode.label)
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The files kept on this phone, and those that couldn't sync. */
@Composable
fun OfflineFilesScreen(session: Session.SignedIn, model: SessionViewModel, onBack: () -> Unit) {
    val changes by ClientHolder.syncChanges.collectAsState()
    val offline by produceState<OfflineStatus?>(null, changes) {
        value = runCatching { session.client.offlineStatus() }.getOrNull()
    }
    LaunchedEffect(Unit) { model.syncNow() }
    SubPageScaffold(title = "Offline files", onBack = onBack) { padding ->
        Column(modifier = Modifier.padding(padding).fillMaxSize().verticalScroll(rememberScrollState())) {
            offline?.let { status ->
                ListItem(
                    leadingContent = { LeadingIcon(R.drawable.ic_offline_pin) },
                    headlineContent = { Text(offlineCounts(status)) },
                    supportingContent = { Text(offlineSummary(status)) },
                    trailingContent = {
                        if (status.problems.isNotEmpty()) {
                            TextButton(onClick = { model.syncNow() }) { Text("Try again") }
                        }
                    },
                )
                status.problems.forEach { problem ->
                    ListItem(
                        leadingContent = {
                            IconTile(
                                R.drawable.ic_error_outline,
                                MaterialTheme.colorScheme.errorContainer,
                                MaterialTheme.colorScheme.onErrorContainer,
                            )
                        },
                        headlineContent = { Text(problem.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        supportingContent = { Text(problem.reason, color = MaterialTheme.colorScheme.error) },
                    )
                }
            }
        }
    }
}

/** The local folders that upload to the server. */
@Composable
fun UploadFoldersScreen(session: Session.SignedIn, model: SessionViewModel, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val active = ClientHolder.active ?: return
    val auto = active.autoUpload
    val changes by ClientHolder.syncChanges.collectAsState()
    var refresh by remember { mutableStateOf(0) }
    var dialog by remember { mutableStateOf<SyncDialog?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val sources = remember(changes, refresh) { auto.sources() }
    val current = remember(changes) { auto.current() }

    val pickFolder = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri ->
        if (uri != null) {
            // Without this, the permission ends when the app does.
            context.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
            val picked = PickedSource(SourceKind.FOLDER, uri.toString(), folderLabel(uri))
            dialog = SyncDialog.ChooseTarget(picked)
        }
    }
    val askPhotos = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { granted ->
        // Location is extra, and a photo-picker selection is a grant too.
        if (granted.any { (permission, ok) ->
                ok && permission != android.Manifest.permission.ACCESS_MEDIA_LOCATION
            }
        ) {
            val camera = PickedSource(SourceKind.PHOTOS, CAMERA_HANDLE, "Camera photos and videos")
            dialog = SyncDialog.ChooseTarget(camera)
        } else {
            error = "Home Space needs access to your photos to upload them."
        }
    }

    SubPageScaffold(
        title = "Folders that upload",
        onBack = onBack,
        actions = {
            if (sources.isNotEmpty()) {
                IconButton(onClick = model::syncNow) {
                    Icon(painterResource(R.drawable.ic_sync), contentDescription = "Sync now")
                }
            }
        },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = { dialog = SyncDialog.AddSource },
                icon = { Icon(painterResource(R.drawable.ic_add), contentDescription = null) },
                text = { Text("Add folder") },
                modifier = Modifier.semantics { contentDescription = "Add folder" },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .padding(padding)
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(start = 16.dp, top = 8.dp, end = 16.dp, bottom = FOLDERS_END_PADDING),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (model.waitingForWifi && sources.isNotEmpty()) WaitingForWifiCard()
            Text(
                "New and changed files go up. Changes on the server never affect your phone.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            sources.forEach { source ->
                SourceCard(
                    source = source,
                    current = current?.takeIf { it.sourceId == source.id },
                    onChangeTarget = { dialog = SyncDialog.ChangeTarget(source) },
                    onRemove = { dialog = SyncDialog.Remove(source) },
                )
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }

    when (val shown = dialog) {
        null -> Unit
        SyncDialog.AddSource -> AlertDialog(
            onDismissRequest = { dialog = null },
            title = { Text("What should upload?") },
            text = {
                Column {
                    TextButton(onClick = {
                        dialog = null
                        askPhotos.launch(cameraPermissions().toTypedArray())
                    }) { Text("Camera photos and videos") }
                    TextButton(onClick = {
                        dialog = null
                        pickFolder.launch(null)
                    }) { Text("Choose a folder") }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Cancel") } },
        )
        is SyncDialog.ChooseTarget -> ServerFolderDialog(
            title = "Upload “${shown.source.label}” to",
            confirm = "Add",
            client = session.client,
            askUploadExisting = true,
            onDismiss = { dialog = null },
            onSessionEnded = model::onSessionEnded,
            onChoose = { folder, label, uploadExisting ->
                auto.addSource(
                    shown.source.kind,
                    shown.source.handle,
                    shown.source.label,
                    folder,
                    label,
                    uploadExisting,
                )
                dialog = null
                refresh++
                model.syncNow()
            },
        )
        is SyncDialog.ChangeTarget -> ServerFolderDialog(
            title = "Upload “${shown.source.label}” to",
            confirm = "Change",
            client = session.client,
            askUploadExisting = false,
            onDismiss = { dialog = null },
            onSessionEnded = model::onSessionEnded,
            onChoose = { folder, label, _ ->
                auto.setSourceTarget(shown.source.id, folder, label)
                dialog = null
                refresh++
                model.syncNow()
            },
        )
        is SyncDialog.Remove -> AlertDialog(
            onDismissRequest = { dialog = null },
            title = { Text("Stop uploading “${shown.source.label}”?") },
            text = { Text("Files already uploaded stay on the server. Your phone isn't changed.") },
            confirmButton = {
                TextButton(onClick = {
                    dialog = null
                    scope.launch {
                        try {
                            auto.removeSource(shown.source.id)
                            releaseFolderPermission(context, shown.source)
                        } catch (e: CoreException) {
                            error = e.fileMessage()
                        }
                        refresh++
                        model.syncNow()
                    }
                }) { Text("Stop uploading") }
            },
            dismissButton = { TextButton(onClick = { dialog = null }) { Text("Cancel") } },
        )
    }

    LaunchedEffect(Unit) { model.syncNow() }
}

@Composable
private fun WaitingForWifiCard() {
    Card(
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.secondaryContainer,
            contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        ),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier.padding(16.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(painterResource(R.drawable.ic_wifi_off), contentDescription = null)
            Text("Waiting for Wi-Fi", style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun SourceCard(
    source: UploadSource,
    current: CurrentUpload?,
    onChangeTarget: () -> Unit,
    onRemove: () -> Unit,
) {
    val failing = source.problem != null || source.lastError != null
    OutlinedCard(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                SourceTile(source.kind)
                Column(modifier = Modifier.weight(1f)) {
                    Text(source.label, style = MaterialTheme.typography.titleMedium)
                    Text(
                        "To ${source.targetLabel}",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                SourceChip(source, current)
            }
            if (current != null) ProgressLine(currentProgress(current), modifier = Modifier.fillMaxWidth())
            Text(
                sourceStatus(source),
                style = MaterialTheme.typography.bodyMedium.tabular(),
                color = if (failing) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
            ) {
                TextButton(onClick = onRemove) { Text("Remove") }
                OutlinedButton(onClick = onChangeTarget) { Text("Change folder") }
            }
        }
    }
}

private fun offlineCounts(status: OfflineStatus): String {
    val items = if (status.kept == 1uL) "1 item" else "${status.kept} items"
    return if (status.problems.isEmpty()) items else "$items · ${status.problems.size} couldn't sync"
}

private fun offlineSummary(status: OfflineStatus): String = when {
    status.unreachable -> "Can't reach the server. Trying again by itself."
    status.pending > 0u -> "${status.pending} waiting to sync"
    status.kept == 0uL -> "Files you open and folders you make available offline are kept here."
    else -> "Up to date"
}

@Composable
internal fun sourceStatus(source: UploadSource): String = when {
    source.problem != null -> source.problem!!
    source.lastError != null -> "${source.pending} to go. ${source.lastError}"
    source.pending > 0u -> "${source.pending} to go"
    else -> source.lastSyncedAt?.let { "Up to date, ${localDate(it)}" } ?: "Waiting to sync"
}

internal fun needsNotificationPermission(context: Context): Boolean =
    Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
        context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
        PackageManager.PERMISSION_GRANTED

private fun appBatterySettings(context: Context): Intent =
    // The per-app page, not the prompt for an exemption, which Google Play restricts.
    Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))

private fun folderLabel(tree: Uri): String =
    DocumentsContract.getTreeDocumentId(tree).substringAfterLast(':').substringAfterLast('/')
        .ifEmpty { "Folder" }

private fun releaseFolderPermission(context: Context, source: UploadSource) {
    if (source.kind == SourceKind.FOLDER) {
        runCatching {
            context.contentResolver.releasePersistableUriPermission(
                Uri.parse(source.handle),
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
    }
}
