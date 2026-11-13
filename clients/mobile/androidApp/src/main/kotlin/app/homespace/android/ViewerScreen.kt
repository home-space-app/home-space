package app.homespace.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry
import uniffi.homespace_core.TextFile
import uniffi.homespace_core.ViewerInfo
import uniffi.homespace_core.ViewerKind

private const val NOT_UTF8 = "This file is not UTF-8 text, so it can only be viewed."
private val CONTENT_PADDING = 16.dp

/**
 * A file open in a built-in viewer. It lives in the view model, so it survives rotation.
 * Each [ViewerKind] has its own state and screen; [OpenViewer.of] picks them.
 */
sealed interface OpenViewer {
    val entry: Entry

    /** Reads what the viewer shows. */
    fun load(scope: CoroutineScope, client: CoreClient, onSessionEnded: () -> Unit)

    companion object {
        /** The state of the viewer `info` names, for `entry`. */
        fun of(entry: Entry, info: ViewerInfo): OpenViewer = when (info.kind) {
            ViewerKind.TEXT -> TextViewerState(entry, info.editable)
        }
    }
}

/** The text viewer: read-only until Edit, then a plain editor with Cancel and Save. */
class TextViewerState(override val entry: Entry, private val viewerEditable: Boolean) : OpenViewer {
    var file by mutableStateOf<TextFile?>(null)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    var editing by mutableStateOf(false)
        private set
    var draft by mutableStateOf("")
    var saving by mutableStateOf(false)
        private set

    /** Whether Edit is offered: content that is not UTF-8 would not survive saving. */
    val canEdit get() = viewerEditable && file?.editable == true

    /** Whether the editor holds text that differs from the file. */
    val changed get() = editing && draft != file?.text

    override fun load(scope: CoroutineScope, client: CoreClient, onSessionEnded: () -> Unit) {
        error = null
        scope.launch {
            try {
                file = client.readText(entry.id)
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
            }
        }
    }

    fun edit() {
        val current = file ?: return
        if (!canEdit) return
        draft = current.text
        error = null
        editing = true
    }

    fun cancel() {
        editing = false
        error = null
    }

    /** Stores the draft as the file's content; the previous content becomes a version. */
    fun save(scope: CoroutineScope, client: CoreClient, onSaved: () -> Unit, onSessionEnded: () -> Unit) {
        val current = file ?: return
        if (saving) return
        val text = draft
        saving = true
        error = null
        scope.launch {
            try {
                val saved = client.saveText(entry.id, text, current.format)
                file = current.copy(entry = saved, text = text)
                editing = false
                onSaved()
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
            } finally {
                saving = false
            }
        }
    }
}

/** The built-in viewer that is open, full screen. */
@Composable
fun ViewerScreen(session: Session.SignedIn, model: SessionViewModel, viewer: OpenViewer) {
    when (viewer) {
        is TextViewerState -> TextViewerScreen(session, model, viewer)
    }
}

@Composable
private fun TextViewerScreen(session: Session.SignedIn, model: SessionViewModel, state: TextViewerState) {
    var confirmDiscard by rememberSaveable { mutableStateOf(false) }
    val back = {
        when {
            state.saving -> Unit
            state.changed -> confirmDiscard = true
            state.editing -> state.cancel()
            else -> model.closeViewer()
        }
    }
    SubPageScaffold(
        title = state.file?.entry?.name ?: state.entry.name,
        onBack = back,
        actions = {
            if (state.editing) {
                TextButton(onClick = state::cancel, enabled = !state.saving) { Text("Cancel") }
                TextButton(
                    onClick = { model.saveText(session, state) },
                    enabled = !state.saving,
                ) { Text("Save") }
            } else if (state.canEdit) {
                IconButton(onClick = state::edit) {
                    Icon(painterResource(R.drawable.ic_edit), contentDescription = "Edit")
                }
            }
        },
    ) { padding ->
        Column(modifier = Modifier.padding(padding).fillMaxSize().imePadding()) {
            if (state.saving) LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            state.error?.let { message ->
                Text(
                    message,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(horizontal = CONTENT_PADDING, vertical = 8.dp),
                )
            }
            val file = state.file
            when {
                file == null && state.error == null -> Box(Modifier.fillMaxSize(), Alignment.Center) {
                    CircularProgressIndicator()
                }
                file == null -> Unit
                state.editing -> TextEditor(state)
                else -> {
                    if (!file.editable) {
                        Text(
                            NOT_UTF8,
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.padding(horizontal = CONTENT_PADDING, vertical = 8.dp),
                        )
                        HorizontalDivider()
                    }
                    TextContent(file.text)
                }
            }
        }
    }
    if (confirmDiscard) {
        AlertDialog(
            onDismissRequest = { confirmDiscard = false },
            title = { Text("Discard changes?") },
            text = { Text("Your edits to “${state.entry.name}” will be lost.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDiscard = false
                    state.cancel()
                }) { Text("Discard") }
            },
            dismissButton = {
                TextButton(onClick = { confirmDiscard = false }) { Text("Keep editing") }
            },
        )
    }
}

private val monospace @Composable get() = TextStyle(
    fontFamily = FontFamily.Monospace,
    fontSize = MaterialTheme.typography.bodyMedium.fontSize,
    lineHeight = MaterialTheme.typography.bodyMedium.lineHeight,
    color = MaterialTheme.colorScheme.onSurface,
)

/** The text, read-only and selectable; one row per line keeps long files quick to scroll. */
@Composable
private fun TextContent(text: String) {
    val lines = remember(text) { text.split('\n') }
    val style = monospace
    SelectionContainer {
        LazyColumn(modifier = Modifier.fillMaxSize().padding(horizontal = CONTENT_PADDING)) {
            itemsIndexed(lines) { _, line -> Text(line, style = style) }
        }
    }
}

@Composable
private fun TextEditor(state: TextViewerState) {
    BasicTextField(
        value = state.draft,
        onValueChange = { state.draft = it },
        enabled = !state.saving,
        textStyle = monospace,
        cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
        modifier = Modifier.fillMaxSize().padding(CONTENT_PADDING),
    )
}
