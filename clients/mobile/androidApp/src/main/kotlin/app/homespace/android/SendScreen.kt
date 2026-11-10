package app.homespace.android

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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.SendTarget

/** A full-screen dialog to pick the device that gets files, over the screen that asked. */
@Composable
internal fun SendToDeviceDialog(
    summary: String,
    client: CoreClient,
    send: suspend (SendTarget) -> Unit,
    onDismiss: () -> Unit,
    onSent: (SendTarget) -> Unit,
    onSessionEnded: () -> Unit,
) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        SendToDevice(summary, client, send, onDismiss, onSent, onSessionEnded)
    }
}

/**
 * Picks the device that gets what `summary` names: the user's own devices first, then those of
 * other users that are visible to them. `send` does the sending once the user confirms.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SendToDevice(
    summary: String,
    client: CoreClient,
    send: suspend (SendTarget) -> Unit,
    onClose: () -> Unit,
    onSent: (SendTarget) -> Unit,
    onSessionEnded: () -> Unit,
) {
    var targets by remember { mutableStateOf<List<SendTarget>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // Which list is shown: the user's own devices, or other users'.
    var others by remember { mutableStateOf(false) }
    var chosen by remember { mutableStateOf<SendTarget?>(null) }
    var sending by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    // Loaded again whenever a device comes online or goes offline, keeping the device picked.
    val deviceChanges by TransferCenter.devices.collectAsState()
    LaunchedEffect(client, deviceChanges) {
        try {
            val listed = client.sendTargets()
            val first = targets == null
            targets = listed
            val kept = chosen?.let { old -> listed.firstOrNull { it.deviceId == old.deviceId } }
            chosen = kept ?: listed.firstOrNull { it.preferred } ?: listed.firstOrNull { it.own }
            // With no other device of one's own, the other users' are all there is to pick.
            if (first) others = listed.isNotEmpty() && listed.none { it.own }
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
        } catch (e: CoreException) {
            error = e.fileMessage()
        }
    }

    val own = targets.orEmpty().filter { it.own }
    val foreign = targets.orEmpty().filter { !it.own }
    val shown = if (others) foreign else own
    val picked = chosen?.takeIf { it in shown }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Send to device") },
                navigationIcon = {
                    IconButton(onClick = onClose) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                    }
                },
            )
        },
        bottomBar = {
            Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                Box(
                    modifier = Modifier
                        .navigationBarsPadding()
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 12.dp),
                    contentAlignment = Alignment.CenterEnd,
                ) {
                    Button(
                        enabled = picked != null && !sending,
                        onClick = {
                            val target = picked ?: return@Button
                            sending = true
                            error = null
                            scope.launch {
                                try {
                                    send(target)
                                    onSent(target)
                                } catch (_: CoreException.Unauthorized) {
                                    onSessionEnded()
                                } catch (e: CoreException) {
                                    error = e.fileMessage()
                                    sending = false
                                }
                            }
                        },
                    ) {
                        Text(
                            when {
                                sending -> "Sending…"
                                picked != null -> "Send to ${picked.recipient}"
                                else -> "Send"
                            },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        },
    ) { padding ->
        LazyColumn(modifier = Modifier.padding(padding).fillMaxSize()) {
            item(key = "summary") { SummaryCard(summary) }
            item(key = "title") {
                SectionHeader(if (others) "Another user’s device" else "Your devices")
                HorizontalDivider()
            }
            error?.let {
                item(key = "error") {
                    Text(
                        it,
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
            }
            if (others && own.isNotEmpty()) {
                item(key = "back") {
                    NavigationRow(R.drawable.ic_arrow_back, "Back to your devices", chevron = false) {
                        others = false
                    }
                    HorizontalDivider()
                }
            }
            items(shown, key = { it.deviceId }) { target ->
                DeviceRow(target, selected = target == picked, enabled = !sending) { chosen = target }
            }
            if (targets != null && shown.isEmpty() && error == null) {
                item(key = "empty") {
                    Text(
                        if (foreign.isEmpty()) {
                            "There is no other device to send to. Sign in to Home Space on another device first."
                        } else {
                            "You have no other device signed in."
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(16.dp),
                    )
                }
            }
            if (!others && foreign.isNotEmpty()) {
                item(key = "others") {
                    HorizontalDivider()
                    NavigationRow(R.drawable.ic_group, "Another user’s device…", chevron = true) {
                        others = true
                    }
                }
            }
            picked?.let { target ->
                item(key = "note") {
                    Text(
                        target.note,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(16.dp),
                    )
                }
            }
        }
    }
}

/** What is being sent. */
@Composable
private fun SummaryCard(summary: String) {
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
                painterResource(R.drawable.ic_send),
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                summary,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun DeviceRow(target: SendTarget, selected: Boolean, enabled: Boolean, onSelect: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .selectable(selected = selected, enabled = enabled, role = Role.RadioButton, onClick = onSelect)
            .heightIn(min = ROW_MIN_HEIGHT)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconTile(
            platformIcon(target.platform),
            MaterialTheme.colorScheme.surfaceContainerHigh,
            MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Column(modifier = Modifier.weight(1f)) {
            Text(
                target.name,
                style = MaterialTheme.typography.bodyLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                target.status,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        // The whole row takes the tap, so the button only shows the state.
        RadioButton(selected = selected, onClick = null, enabled = enabled)
    }
}

/** A row that changes which devices are listed. */
@Composable
private fun NavigationRow(icon: Int, label: String, chevron: Boolean, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = ROW_MIN_HEIGHT)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconTile(
            icon,
            MaterialTheme.colorScheme.surfaceContainerHigh,
            MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        if (chevron) Chevron()
    }
}
