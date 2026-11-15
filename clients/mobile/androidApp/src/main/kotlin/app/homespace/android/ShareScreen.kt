package app.homespace.android

import android.content.ClipData
import android.content.ClipboardManager
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SelectableDates
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TimePicker
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.material3.rememberTimePickerState
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import java.time.Instant
import java.time.LocalDate
import java.time.LocalTime
import java.time.ZoneId
import java.time.ZoneOffset
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry
import uniffi.homespace_core.ShareAudience
import uniffi.homespace_core.ShareExpiry
import uniffi.homespace_core.ShareForm
import uniffi.homespace_core.ShareFormState
import uniffi.homespace_core.SharePerson
import uniffi.homespace_core.ShareSaved

private const val SAVE = "Save"
private const val SAVING = "Saving…"
private const val STOP_SHARING = "Stop sharing"
private const val COPY_LINK = "Copy link"
private const val NO_LINK_YET = "Save to make the link"
private const val HIDDEN_PASSWORD = "••••••••••"
private const val PEOPLE_WITH_ACCESS = "People with access"
private const val ADD_PEOPLE = "Add people"
private const val CAN_VIEW = "Can view"
private const val OWNER = "Owner"
private const val NO_ONE_FOUND = "No one by that name."
private const val PEOPLE_NOTE =
    "People you add can view and download these items. They can’t share them with anyone else."
private val AVATAR_SIZE = 40.dp
private val ADD_TILE_RADIUS = 12.dp
private val PICKER_HEIGHT = 320.dp

private val AUDIENCES = listOf(
    Triple(ShareAudience.PEOPLE, "People", R.drawable.ic_group),
    Triple(ShareAudience.LINK, "Link", R.drawable.ic_link),
)
private val SECTION_GAP = 20.dp
private val ROW_GAP = 12.dp
private val CHIP_GAP = 8.dp
private val CHIP_CHECK = 18.dp

private val PRESETS = listOf(
    ShareExpiry.DAY to "1 day",
    ShareExpiry.WEEK to "7 days",
    ShareExpiry.MONTH to "30 days",
    ShareExpiry.NEVER to "Never",
)

/**
 * A full-screen dialog that shares `entries` read-only with people on the server, or through a
 * link, over the screen that asked. `owner` is the signed-in user's name.
 */
@Composable
internal fun ShareLinkDialog(
    entries: List<Entry>,
    owner: String,
    client: CoreClient,
    onDismiss: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        ShareLink(entries, owner, client, onDismiss, onSessionEnded)
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun ShareLink(
    entries: List<Entry>,
    owner: String,
    client: CoreClient,
    onClose: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var form by remember { mutableStateOf<ShareForm?>(null) }
    var state by remember { mutableStateOf<ShareFormState?>(null) }
    var password by remember { mutableStateOf("") }
    var reveal by remember { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var confirmStop by remember { mutableStateOf(false) }
    var pickingDate by remember { mutableStateOf(false) }
    var pickedDay by remember { mutableStateOf<LocalDate?>(null) }
    var addingPeople by remember { mutableStateOf(false) }

    LaunchedEffect(client, entries) {
        try {
            val loaded = client.shareForm(entries, owner)
            form = loaded
            state = loaded.state()
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
        } catch (e: CoreException) {
            error = e.fileMessage()
        }
    }

    /** Runs `change` on the form and shows what it leads to. */
    fun edit(change: (ShareForm) -> Unit) {
        val current = form ?: return
        change(current)
        state = current.state()
    }

    fun save() {
        val current = form ?: return
        saving = true
        error = null
        scope.launch {
            try {
                when (current.save()) {
                    ShareSaved.CREATED -> {
                        password = ""
                        state = current.state()
                        saving = false
                    }
                    ShareSaved.UPDATED -> onClose()
                }
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
                saving = false
            }
        }
    }

    val shown = state
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(shown?.title ?: "Share", maxLines = 1, overflow = TextOverflow.Ellipsis)
                },
                navigationIcon = {
                    IconButton(onClick = onClose) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                    }
                },
                actions = {
                    TextButton(enabled = shown != null && !saving, onClick = ::save) {
                        Text(if (saving) SAVING else SAVE)
                    }
                },
            )
        },
    ) { padding ->
        if (shown == null) {
            Column(
                modifier = Modifier.padding(padding).fillMaxSize(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) } ?: CircularProgressIndicator()
            }
            return@Scaffold
        }
        Column(
            modifier = Modifier
                .padding(padding)
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .imePadding()
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(SECTION_GAP),
        ) {
            Text(
                shown.subtitle,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
                AUDIENCES.forEachIndexed { index, (audience, label, icon) ->
                    SegmentedButton(
                        selected = shown.audience == audience,
                        onClick = {
                            error = null
                            edit { it.setAudience(audience) }
                        },
                        shape = SegmentedButtonDefaults.itemShape(index, AUDIENCES.size),
                        icon = {
                            SegmentedButtonDefaults.Icon(active = shown.audience == audience) {
                                Icon(painterResource(icon), contentDescription = null, Modifier.size(CHIP_CHECK))
                            }
                        },
                    ) { Text(label) }
                }
            }
            if (shown.audience == ShareAudience.PEOPLE) {
                PeopleSection(
                    shown,
                    onRemove = { person -> edit { it.removePerson(person.id) } },
                    onAdd = { addingPeople = true },
                )
            } else {
                LinkSection(shown.url) { url ->
                    val clipboard = context.getSystemService(ClipboardManager::class.java)
                    clipboard?.setPrimaryClip(ClipData.newPlainText(COPY_LINK, url))
                    Toast.makeText(context, "Link copied", Toast.LENGTH_SHORT).show()
                }
                HorizontalDivider()
                Column(verticalArrangement = Arrangement.spacedBy(ROW_GAP)) {
                    SectionTitle(R.drawable.ic_lock, "Require a password") {
                        Switch(
                            checked = shown.requirePassword,
                            onCheckedChange = { on ->
                                password = ""
                                edit { it.setRequirePassword(on) }
                            },
                        )
                    }
                    if (shown.editingPassword) {
                        OutlinedTextField(
                            value = password,
                            onValueChange = { typed ->
                                password = typed
                                edit { it.setPassword(typed) }
                            },
                            label = { Text("Password") },
                            singleLine = true,
                            visualTransformation =
                                if (reveal) VisualTransformation.None else PasswordVisualTransformation(),
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                            trailingIcon = {
                                IconButton(onClick = { reveal = !reveal }) {
                                    Icon(
                                        painterResource(if (reveal) R.drawable.ic_visibility_off else R.drawable.ic_visibility),
                                        contentDescription = if (reveal) "Hide password" else "Show password",
                                    )
                                }
                            },
                            modifier = Modifier.fillMaxWidth(),
                        )
                    } else if (shown.offersChangePassword) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(HIDDEN_PASSWORD, modifier = Modifier.weight(1f))
                            OutlinedButton(onClick = { edit { it.changePassword() } }) {
                                Icon(painterResource(R.drawable.ic_key), contentDescription = null)
                                Spacer(Modifier.width(8.dp))
                                Text("Change password")
                            }
                        }
                    }
                }
            }
            HorizontalDivider()
            Column(verticalArrangement = Arrangement.spacedBy(ROW_GAP)) {
                SectionTitle(R.drawable.ic_event, "Expires") {
                    Text(
                        shown.expiresAt?.let { localDate(it) } ?: "Never",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(CHIP_GAP),
                ) {
                    PRESETS.forEach { (expiry, label) ->
                        ExpiryChip(label, shown.expiry == expiry) { edit { it.setExpiry(expiry) } }
                    }
                    ExpiryChip("Pick a date…", shown.expiry == ShareExpiry.DATE) {
                        edit { it.setExpiry(ShareExpiry.DATE) }
                        pickingDate = true
                    }
                }
            }
            if (shown.audience == ShareAudience.PEOPLE) {
                Row(horizontalArrangement = Arrangement.spacedBy(ROW_GAP)) {
                    Icon(
                        painterResource(R.drawable.ic_info),
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        PEOPLE_NOTE,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            if (shown.hasShare) {
                OutlinedButton(
                    enabled = !saving,
                    onClick = { confirmStop = true },
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) {
                    Icon(painterResource(stopIcon(shown.audience)), contentDescription = null)
                    Spacer(Modifier.width(8.dp))
                    Text(STOP_SHARING)
                }
            }
        }
        if (addingPeople) {
            AddPeopleDialog(
                form = form,
                onPick = { person ->
                    addingPeople = false
                    edit { it.addPerson(person.id) }
                },
                onDismiss = { addingPeople = false },
            )
        }
        if (confirmStop) {
            AlertDialog(
                onDismissRequest = { confirmStop = false },
                icon = { Icon(painterResource(stopIcon(shown.audience)), contentDescription = null) },
                title = { Text(shown.stopQuestion) },
                text = { Text(shown.stopNote) },
                confirmButton = {
                    TextButton(
                        enabled = !saving,
                        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                        onClick = {
                            val current = form ?: return@TextButton
                            saving = true
                            scope.launch {
                                try {
                                    current.stop()
                                    onClose()
                                } catch (_: CoreException.Unauthorized) {
                                    onSessionEnded()
                                } catch (e: CoreException) {
                                    error = e.fileMessage()
                                    saving = false
                                    confirmStop = false
                                }
                            }
                        },
                    ) { Text(STOP_SHARING) }
                },
                dismissButton = { TextButton(onClick = { confirmStop = false }) { Text("Cancel") } },
            )
        }
        if (pickingDate) {
            DatePick(
                initial = shown.expiresAt ?: Instant.now(),
                onPicked = { day ->
                    pickingDate = false
                    pickedDay = day
                },
                onDismiss = { pickingDate = false },
            )
        }
        pickedDay?.let { day ->
            TimePick(
                initial = shown.expiresAt ?: Instant.now(),
                onPicked = { time ->
                    pickedDay = null
                    val at = day.atTime(time).atZone(ZoneId.systemDefault()).toInstant()
                    edit { it.pickDate(at) }
                },
                onDismiss = { pickedDay = null },
            )
        }
    }
}

private fun stopIcon(audience: ShareAudience): Int = when (audience) {
    ShareAudience.PEOPLE -> R.drawable.ic_person_remove
    ShareAudience.LINK -> R.drawable.ic_link_off
}

/** Who has access: the owner, everyone added, and the button that adds more. */
@Composable
private fun PeopleSection(shown: ShareFormState, onRemove: (SharePerson) -> Unit, onAdd: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(ROW_GAP)) {
        Text(
            PEOPLE_WITH_ACCESS,
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
        )
        PersonRow(shown.owner, null) {
            Text(OWNER, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        shown.people.forEach { person ->
            PersonRow(person.username, person.addedAt?.let { "Added ${localDate(it)}" }) {
                Text(CAN_VIEW, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                IconButton(onClick = { onRemove(person) }) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove ${person.username}")
                }
            }
        }
        Row(
            modifier = Modifier.fillMaxWidth().clickable(onClick = onAdd).padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(AVATAR_SIZE)
                    .background(MaterialTheme.colorScheme.secondaryContainer, RoundedCornerShape(ADD_TILE_RADIUS)),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    painterResource(R.drawable.ic_person_add),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSecondaryContainer,
                )
            }
            Text(ADD_PEOPLE, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.primary)
        }
    }
}

@Composable
private fun PersonRow(name: String, detail: String?, end: @Composable () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Avatar(name, AVATAR_SIZE, MaterialTheme.typography.titleMedium)
        Column(modifier = Modifier.weight(1f)) {
            Text(name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            detail?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        end()
    }
}

/** Finds the users to share with by name. */
@Composable
private fun AddPeopleDialog(form: ShareForm?, onPick: (SharePerson) -> Unit, onDismiss: () -> Unit) {
    var query by remember { mutableStateOf("") }
    val found = remember(form, query) { form?.candidates(query) ?: emptyList() }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(painterResource(R.drawable.ic_person_add), contentDescription = null) },
        title = { Text(ADD_PEOPLE) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(ROW_GAP)) {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    placeholder = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                if (found.isEmpty()) {
                    Text(NO_ONE_FOUND, color = MaterialTheme.colorScheme.onSurfaceVariant)
                } else {
                    LazyColumn(modifier = Modifier.heightIn(max = PICKER_HEIGHT)) {
                        items(found, key = { it.id }) { person ->
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable { onPick(person) }
                                    .padding(vertical = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(16.dp),
                            ) {
                                Avatar(person.username, 32.dp, MaterialTheme.typography.titleSmall)
                                Text(person.username, style = MaterialTheme.typography.bodyLarge)
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The link, or a note that saving makes it, with the button that copies it. */
@Composable
private fun LinkSection(url: String?, onCopy: (String) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(ROW_GAP)) {
        OutlinedTextField(
            value = url ?: "",
            onValueChange = {},
            readOnly = true,
            singleLine = true,
            placeholder = { Text(NO_LINK_YET) },
            leadingIcon = { Icon(painterResource(R.drawable.ic_link), contentDescription = null) },
            modifier = Modifier.fillMaxWidth(),
        )
        FilledTonalButton(
            enabled = url != null,
            onClick = { url?.let(onCopy) },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Icon(painterResource(R.drawable.ic_content_copy), contentDescription = null)
            Spacer(Modifier.width(8.dp))
            Text(COPY_LINK)
        }
    }
}

@Composable
private fun SectionTitle(icon: Int, title: String, end: @Composable () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(ROW_GAP)) {
        Icon(painterResource(icon), contentDescription = null)
        Text(title, style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
        end()
    }
}

@Composable
private fun ExpiryChip(label: String, selected: Boolean, onClick: () -> Unit) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = { Text(label) },
        leadingIcon = if (selected) {
            { Icon(painterResource(R.drawable.ic_check), contentDescription = null, Modifier.size(CHIP_CHECK)) }
        } else {
            null
        },
    )
}

/** Picks the day a link stops working; days before today can't be picked. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DatePick(initial: Instant, onPicked: (LocalDate) -> Unit, onDismiss: () -> Unit) {
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    // The picker works in UTC midnights.
    val initialMillis = initial.atZone(zone).toLocalDate().atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli()
    val state = rememberDatePickerState(
        initialSelectedDateMillis = initialMillis,
        selectableDates = object : SelectableDates {
            override fun isSelectableDate(utcTimeMillis: Long): Boolean =
                !Instant.ofEpochMilli(utcTimeMillis).atZone(ZoneOffset.UTC).toLocalDate().isBefore(today)
        },
    )
    DatePickerDialog(
        onDismissRequest = onDismiss,
        confirmButton = {
            TextButton(
                enabled = state.selectedDateMillis != null,
                onClick = {
                    state.selectedDateMillis?.let { millis ->
                        onPicked(Instant.ofEpochMilli(millis).atZone(ZoneOffset.UTC).toLocalDate())
                    }
                },
            ) { Text("Next") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    ) {
        DatePicker(state = state)
    }
}

/** Picks the time of day a link stops working. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TimePick(initial: Instant, onPicked: (LocalTime) -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val start = initial.atZone(ZoneId.systemDefault()).toLocalTime()
    val state = rememberTimePickerState(
        initialHour = start.hour,
        initialMinute = start.minute,
        is24Hour = android.text.format.DateFormat.is24HourFormat(context),
    )
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Expires at") },
        text = { TimePicker(state = state) },
        confirmButton = {
            TextButton(onClick = { onPicked(LocalTime.of(state.hour, state.minute)) }) { Text("OK") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
