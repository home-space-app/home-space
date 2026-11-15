package app.homespace.android

import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PagerState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.SharedEntry
import uniffi.homespace_core.ViewerKind
import uniffi.homespace_core.viewerFor

private val CONTENT_PADDING = 16.dp

/** A file shared with the user, open read-only in its built-in viewer; [siblings] are the files beside it. */
@Composable
fun SharedViewerScreen(
    session: Session.SignedIn,
    file: SharedEntry,
    siblings: List<SharedEntry>,
    onBack: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    when (viewerFor(file.mime, file.size)?.kind) {
        ViewerKind.IMAGE -> SharedImages(session, file, siblings, onBack, onSessionEnded)
        else -> SharedText(session, file, onBack, onSessionEnded)
    }
}

private sealed interface Reading {
    data object Loading : Reading
    data class Read(val text: String) : Reading
    data class Failed(val message: String) : Reading
}

@Composable
private fun SharedText(session: Session.SignedIn, file: SharedEntry, onBack: () -> Unit, onSessionEnded: () -> Unit) {
    val reading by produceState<Reading>(Reading.Loading, file.shareId, file.id) {
        value = try {
            Reading.Read(session.client.readSharedText(file))
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
            Reading.Loading
        } catch (e: CoreException) {
            Reading.Failed(e.fileMessage())
        }
    }
    SubPageScaffold(title = file.name, onBack = onBack) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when (val shown = reading) {
                Reading.Loading -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                is Reading.Read -> TextContent(shown.text)
                is Reading.Failed -> Text(
                    shown.message,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(horizontal = CONTENT_PADDING, vertical = 8.dp),
                )
            }
        }
    }
}

@Composable
private fun SharedImages(
    session: Session.SignedIn,
    file: SharedEntry,
    siblings: List<SharedEntry>,
    onBack: () -> Unit,
    onSessionEnded: () -> Unit,
) {
    val pictures = remember(file, siblings) {
        siblings.filter { viewerFor(it.mime, it.size)?.kind == ViewerKind.IMAGE }.ifEmpty { listOf(file) }
    }
    val start = pictures.indexOfFirst { it.id == file.id }.coerceAtLeast(0)
    val pager = remember(pictures) { PagerState(currentPage = start) { pictures.size } }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    val current = pictures.getOrNull(pager.currentPage) ?: file
    val step = { offset: Int ->
        val target = pager.currentPage + offset
        if (target in pictures.indices) scope.launch { pager.animateScrollToPage(target) }
    }
    SubPageScaffold(
        title = current.name,
        onBack = onBack,
        actions = {
            if (pictures.size > 1) {
                IconButton(onClick = { step(-1) }, enabled = pager.currentPage > 0) {
                    Icon(painterResource(R.drawable.ic_chevron_left), contentDescription = "Previous image")
                }
                Text("${pager.currentPage + 1} / ${pictures.size}", style = MaterialTheme.typography.labelLarge)
                IconButton(onClick = { step(1) }, enabled = pager.currentPage < pictures.lastIndex) {
                    Icon(painterResource(R.drawable.ic_chevron_right), contentDescription = "Next image")
                }
            }
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .padding(padding)
                .fillMaxSize()
                .focusRequester(focus)
                .focusable()
                .onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                    when (event.key) {
                        Key.Spacebar, Key.DirectionRight -> { step(1); true }
                        Key.DirectionLeft -> { step(-1); true }
                        else -> false
                    }
                },
        ) {
            HorizontalPager(state = pager, key = { pictures[it].id }, modifier = Modifier.fillMaxSize()) { page ->
                val picture = pictures[page]
                ImagePage(
                    key = picture.shareId to picture.id,
                    name = picture.name,
                    load = { session.client.readSharedImage(picture) },
                    onSessionEnded = onSessionEnded,
                )
            }
        }
    }
    LaunchedEffect(Unit) { focus.requestFocus() }
}
