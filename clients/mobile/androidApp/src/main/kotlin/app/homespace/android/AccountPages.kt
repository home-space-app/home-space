package app.homespace.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.QueuedUploadState
import uniffi.homespace_core.TransferStatus
import uniffi.homespace_core.UserDevice

private const val PLATFORM_ANDROID = "android"
private const val PLATFORM_IOS = "ios"
private const val PLATFORM_LINUX = "desktop-linux"
private const val PLATFORM_MACOS = "desktop-macos"
private const val PLATFORM_WINDOWS = "desktop-windows"
private const val PLATFORM_WEB = "web"

private val PROFILE_AVATAR_SIZE = 72.dp
private val BUTTON_HEIGHT = 48.dp

/** The top-level destinations, in the navigation bar. */
private enum class Tab(val title: String, val icon: Int, val selectedIcon: Int) {
    FILES("Files", R.drawable.ic_folder, R.drawable.ic_folder_filled),
    TRANSFERS("Transfers", R.drawable.ic_swap_vert, R.drawable.ic_swap_vert_filled),
    SETTINGS("Settings", R.drawable.ic_settings, R.drawable.ic_settings_filled),
}

/** The screens that cover the navigation bar, and go back to where they were opened from. */
enum class SubPage { PROFILE, UPLOAD_FOLDERS, OFFLINE_FILES }

/** The folder browser and the screens around it, which end with the session. */
@Composable
fun SignedInScreen(session: Session.SignedIn, model: SessionViewModel) {
    var tab by rememberSaveable { mutableStateOf(Tab.FILES) }
    var page by rememberSaveable { mutableStateOf<SubPage?>(null) }
    // Keeps the scroll position of the list for the way back.
    val saved = rememberSaveableStateHolder()
    val navigationBar = @Composable {
        HomeSpaceNavigationBar(
            selected = tab,
            // What needs the user: uploads that failed, and transfers waiting to be received.
            failedUploads = model.uploads.count { it.state == QueuedUploadState.FAILED } +
                model.received.count { it.status == TransferStatus.READY && it.id !in TransferCenter.progress },
            onSelect = { tab = it },
        )
    }
    val closePage = { page = null }
    // A transfer's notification opens the list it is in.
    if (model.showTransfers) {
        LaunchedEffect(Unit) {
            page = null
            tab = Tab.TRANSFERS
            model.showTransfers = false
        }
    }
    when (page) {
        SubPage.PROFILE -> ProfileScreen(session, model, closePage)
        SubPage.UPLOAD_FOLDERS -> UploadFoldersScreen(session, model, closePage)
        SubPage.OFFLINE_FILES -> OfflineFilesScreen(session, model, closePage)
        null -> {
            // The other tabs go back to Files, and from there back leaves the app.
            BackHandler(enabled = tab != Tab.FILES) { tab = Tab.FILES }
            when (tab) {
                Tab.FILES -> saved.SaveableStateProvider("browse") {
                    BrowseScreen(
                        session = session,
                        model = model,
                        navigationBar = navigationBar,
                        onOpenTransfers = { tab = Tab.TRANSFERS },
                        onOpenProfile = { page = SubPage.PROFILE },
                    )
                }
                Tab.TRANSFERS -> TransfersScreen(session, model, navigationBar, onOpenPage = { page = it })
                Tab.SETTINGS -> SettingsScreen(session, model, navigationBar, onOpenPage = { page = it })
            }
        }
    }

    // Here, because signing out and removing a copy are asked for from more than one screen.
    model.removalWarning?.let { warning ->
        UnsentEditsDialog(
            title = "Remove “${warning.entry.name}” from this phone?",
            names = warning.names,
            confirm = "Remove",
            onConfirm = { model.removeOfflineCopy(session, warning.entry, confirmed = true) },
            onDismiss = { model.removalWarning = null },
        )
    }
    model.signOutWarning?.let { names ->
        UnsentEditsDialog(
            title = "Sign out?",
            names = names,
            confirm = "Sign out",
            onConfirm = model::signOut,
            onDismiss = { model.signOutWarning = null },
        )
    }
}

@Composable
private fun HomeSpaceNavigationBar(selected: Tab, failedUploads: Int, onSelect: (Tab) -> Unit) {
    NavigationBar {
        Tab.entries.forEach { tab ->
            val isSelected = tab == selected
            NavigationBarItem(
                selected = isSelected,
                onClick = { onSelect(tab) },
                label = { Text(tab.title) },
                colors = NavigationBarItemDefaults.colors(selectedTextColor = MaterialTheme.colorScheme.onSurface),
                icon = {
                    val icon = @Composable {
                        Icon(
                            painterResource(if (isSelected) tab.selectedIcon else tab.icon),
                            contentDescription = null,
                        )
                    }
                    if (tab == Tab.TRANSFERS && failedUploads > 0) {
                        BadgedBox(badge = { Badge { Text(failedUploads.toString()) } }) { icon() }
                    } else {
                        icon()
                    }
                },
            )
        }
    }
}

/** A screen opened from a tab: a title and the way back. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SubPageScaffold(
    title: String,
    onBack: () -> Unit,
    actions: @Composable () -> Unit = {},
    floatingActionButton: @Composable () -> Unit = {},
    content: @Composable (PaddingValues) -> Unit,
) {
    BackHandler(onBack = onBack)
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back")
                    }
                },
                actions = { actions() },
            )
        },
        floatingActionButton = floatingActionButton,
        content = content,
    )
}

/** Who is signed in and where, and the user's other devices. */
@Composable
private fun ProfileScreen(session: Session.SignedIn, model: SessionViewModel, onBack: () -> Unit) {
    val devices by produceState<List<UserDevice>?>(null, session.client, model.devicesKey) {
        value = try {
            session.client.devices()
        } catch (_: CoreException.Unauthorized) {
            model.onSessionEnded()
            null
        } catch (_: CoreException) {
            emptyList()
        }
    }
    SubPageScaffold(title = "Profile", onBack = onBack) { padding ->
        Column(modifier = Modifier.padding(padding).fillMaxSize()) {
            LazyColumn(modifier = Modifier.weight(1f)) {
                item { ProfileHeader(session) }
                item { HorizontalDivider() }
                if (devices?.isNotEmpty() == true) {
                    item { SectionHeader("Devices") }
                    items(devices.orEmpty(), key = { it.id }) { device -> DeviceRow(device) }
                }
            }
            OutlinedButton(
                onClick = { model.requestSignOut(session) },
                colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                modifier = Modifier.fillMaxWidth().padding(24.dp).height(BUTTON_HEIGHT),
            ) {
                Icon(
                    painterResource(R.drawable.ic_logout),
                    contentDescription = null,
                    modifier = Modifier.padding(end = 8.dp).size(ButtonDefaults.IconSize),
                )
                Text("Sign out")
            }
        }
    }
}

@Composable
private fun ProfileHeader(session: Session.SignedIn) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Avatar(session.user.username, PROFILE_AVATAR_SIZE, MaterialTheme.typography.headlineMedium)
        Text(session.user.username, style = MaterialTheme.typography.headlineSmall)
        Surface(
            shape = RoundedCornerShape(8.dp),
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        ) {
            Row(
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    painterResource(R.drawable.ic_dns),
                    contentDescription = "Server",
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(18.dp),
                )
                Text(
                    serverLabel(session.client.baseUrl()),
                    style = MaterialTheme.typography.labelLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
private fun DeviceRow(device: UserDevice) {
    val platform = platformLabel(device.platform)
    val status = when {
        device.current -> "This phone · $platform"
        !device.signedIn -> "$platform · signed out"
        else -> "$platform · seen ${localDate(device.lastSeenAt)}"
    }
    ListItem(
        leadingContent = { LeadingIcon(platformIcon(device.platform)) },
        headlineContent = { Text(device.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = {
            Text(status, style = MaterialTheme.typography.bodyMedium.tabular(), maxLines = 1, overflow = TextOverflow.Ellipsis)
        },
        trailingContent = if (device.current) {
            { StatusChip("Current", ChipTone.NEUTRAL) }
        } else {
            null
        },
    )
}

private fun platformLabel(platform: String): String = when (platform) {
    PLATFORM_ANDROID -> "Android"
    PLATFORM_IOS -> "iOS"
    PLATFORM_LINUX -> "Linux"
    PLATFORM_MACOS -> "macOS"
    PLATFORM_WINDOWS -> "Windows"
    PLATFORM_WEB -> "Web"
    else -> platform
}

internal fun platformIcon(platform: String): Int = when (platform) {
    PLATFORM_ANDROID, PLATFORM_IOS -> R.drawable.ic_smartphone
    PLATFORM_WEB -> R.drawable.ic_language
    else -> R.drawable.ic_laptop
}

/** Asks before something goes that holds edits not on the server yet. */
@Composable
private fun UnsentEditsDialog(
    title: String,
    names: List<String>,
    confirm: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    val shown = names.take(MAX_NAMES_SHOWN).joinToString(", ")
    val more = names.size - MAX_NAMES_SHOWN
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Text(
                "Edits to $shown${if (more > 0) " and $more more" else ""} aren't on the " +
                    "server yet and will be lost.",
            )
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

private const val MAX_NAMES_SHOWN = 3
