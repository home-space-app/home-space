package app.homespace.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.toMutableStateList
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.DeviceAudience
import uniffi.homespace_core.OtherUser
import uniffi.homespace_core.UserDevice
import uniffi.homespace_core.audienceChoices
import uniffi.homespace_core.removeDeviceHint

private const val VISIBILITY_NOTE =
    "New devices start visible only to you. Your other devices keep their own setting."
private const val NO_OTHER_USERS = "There are no other users on this server."
private const val LOADING_USERS = "Loading users…"
private const val SIGN_OUT_HINT = "It has to sign in again to sync."

/** What the user asked to do to one of their devices, waiting for them to confirm it. */
internal enum class DeviceEnding(val label: String) {
    SIGN_OUT("Sign out"),

    /** Signs it out and takes it off the list. */
    REMOVE("Remove"),
}

internal fun audienceIcon(audience: DeviceAudience): Int = when (audience) {
    DeviceAudience.HIDDEN -> R.drawable.ic_visibility_off
    DeviceAudience.USERS -> R.drawable.ic_group
    DeviceAudience.EVERYONE -> R.drawable.ic_language
    DeviceAudience.OWNER -> R.drawable.ic_person
}

/** Renames one of the user's devices. */
@Composable
internal fun RenameDeviceDialog(
    client: CoreClient,
    device: UserDevice,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    NameDialog(
        title = "Rename device",
        confirm = "Rename",
        initial = device.name,
        save = { name -> client.renameDevice(device.id, name) },
        onDismiss = onDismiss,
        onDone = onDone,
        onSessionEnded = onSessionEnded,
    )
}

/** Asks who can see one of the user's devices and send files to it. */
@Composable
internal fun DeviceVisibilityDialog(
    client: CoreClient,
    device: UserDevice,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    val choices = remember { audienceChoices() }
    var audience by remember { mutableStateOf(device.audience) }
    val picked = remember { device.visibleTo.map { it.id }.toMutableStateList() }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val users by produceState<List<OtherUser>?>(null, client) {
        value = try {
            client.otherUsers()
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
            emptyList()
        } catch (e: CoreException) {
            error = e.fileMessage()
            emptyList()
        }
    }
    // Visible to specific users with nobody picked would be the same as only you.
    val canSave = !saving && (audience != DeviceAudience.USERS || picked.isNotEmpty())
    val save = {
        saving = true
        error = null
        scope.launch {
            try {
                client.setDeviceAudience(device.id, audience, picked.toList())
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
        title = {
            Column {
                Text("Who can see")
                Text("${device.name}?", maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Column(modifier = Modifier.selectableGroup()) {
                    choices.forEach { choice ->
                        val selected = choice.audience == audience
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .selectable(
                                    selected = selected,
                                    role = Role.RadioButton,
                                    onClick = { audience = choice.audience },
                                )
                                .padding(vertical = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RadioButton(selected = selected, onClick = null)
                            Column(modifier = Modifier.padding(start = 16.dp)) {
                                Text(choice.title, style = MaterialTheme.typography.bodyLarge)
                                Text(
                                    choice.hint,
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                        if (selected && choice.audience == DeviceAudience.USERS) {
                            UserChecklist(users, picked, onToggle = { id ->
                                if (!picked.remove(id)) picked.add(id)
                            })
                        }
                    }
                }
                Text(
                    VISIBILITY_NOTE,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                error?.let {
                    Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = save, enabled = canSave) { Text(if (saving) WORKING else "Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The other users, each with a box that makes the device visible to them. */
@Composable
private fun UserChecklist(users: List<OtherUser>?, picked: List<String>, onToggle: (String) -> Unit) {
    Column(modifier = Modifier.padding(start = 40.dp)) {
        when {
            users == null -> Hint(LOADING_USERS)
            users.isEmpty() -> Hint(NO_OTHER_USERS)
            else -> users.forEach { user ->
                val checked = user.id in picked
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .toggleable(value = checked, role = Role.Checkbox, onValueChange = { onToggle(user.id) })
                        .padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Checkbox(checked = checked, onCheckedChange = null)
                    Avatar(user.username, 32.dp, MaterialTheme.typography.titleSmall)
                    Text(user.username, style = MaterialTheme.typography.bodyLarge)
                }
            }
        }
    }
}

@Composable
private fun Hint(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(vertical = 8.dp),
    )
}

/** Asks before signing a device out or removing it, then does it. */
@Composable
internal fun EndDeviceDialog(
    client: CoreClient,
    device: UserDevice,
    ending: DeviceEnding,
    onDismiss: () -> Unit,
    onDone: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    var working by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val (question, hint) = when (ending) {
        DeviceEnding.SIGN_OUT -> device.signOutQuestion to SIGN_OUT_HINT
        DeviceEnding.REMOVE -> device.removeQuestion to remember { removeDeviceHint() }
    }
    val confirm = {
        working = true
        error = null
        scope.launch {
            try {
                when (ending) {
                    DeviceEnding.SIGN_OUT -> client.signOutDevice(device.id)
                    DeviceEnding.REMOVE -> client.removeDevice(device.id)
                }
                onDone()
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
                working = false
            }
        }
        Unit
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(question) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(hint)
                error?.let {
                    Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = confirm,
                enabled = !working,
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
            ) { Text(if (working) WORKING else ending.label) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
