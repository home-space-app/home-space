package app.homespace.android

import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import androidx.compose.animation.core.AnimationState
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateDecay
import androidx.compose.animation.rememberSplineBasedDecay
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.isSpecified
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import java.io.File
import java.io.IOException
import java.util.UUID
import kotlin.math.roundToInt
import kotlin.math.sqrt
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry

private const val NOT_A_PDF = "This PDF can't be opened on this phone."
private const val PROTECTED = "This PDF is protected with a password. Download it to open it."
private const val MAX_ZOOM = 4f
private const val DOUBLE_TAP_ZOOM = 2.5f
/** The most pixels a page is rendered with, to bound the memory a zoomed page takes. */
private const val MAX_PAGE_PIXELS = 8_000_000f
private val PAGE_GAP = 8.dp

/** The PDF viewer of one of the user's own files; the screen reads the document. */
class PdfViewerState(override val entry: Entry) : OpenViewer {
    override fun load(scope: CoroutineScope, client: CoreClient, onSessionEnded: () -> Unit) = Unit
}

@Composable
internal fun PdfViewerScreen(session: Session.SignedIn, model: SessionViewModel, state: PdfViewerState) {
    PdfScreen(
        key = state.entry.id,
        title = state.entry.name,
        onBack = model::closeViewer,
        read = { session.client.readPdf(state.entry.id).bytes },
        onSessionEnded = model::onSessionEnded,
    )
}

private sealed interface Opening {
    data object Loading : Opening
    data class Opened(val document: PdfPages) : Opening
    data class Failed(val message: String) : Opening
}

/**
 * A PDF, its pages one under another, fitted to the width, pinch to zoom. [read] fetches the
 * document; it is written to the app's cache for [PdfRenderer], which needs a file, and deleted
 * when the screen goes.
 */
@Composable
internal fun PdfScreen(
    key: Any,
    title: String,
    onBack: () -> Unit,
    read: suspend () -> ByteArray,
    onSessionEnded: () -> Unit,
) {
    val cache = LocalContext.current.cacheDir
    val opening by produceState<Opening>(Opening.Loading, key) {
        value = try {
            val bytes = read()
            Opening.Opened(withContext(Dispatchers.IO) { PdfPages.open(cache, bytes) })
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
            Opening.Loading
        } catch (e: CoreException) {
            Opening.Failed(e.fileMessage())
        } catch (_: SecurityException) {
            Opening.Failed(PROTECTED)
        } catch (_: IOException) {
            Opening.Failed(NOT_A_PDF)
        }
        awaitDispose { (value as? Opening.Opened)?.document?.close() }
    }
    SubPageScaffold(title = title, onBack = onBack) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when (val shown = opening) {
                Opening.Loading -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                is Opening.Opened -> Pages(shown.document)
                is Opening.Failed -> Text(
                    shown.message,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.align(Alignment.Center).padding(24.dp),
                )
            }
        }
    }
}

@Composable
private fun Pages(document: PdfPages) {
    var zoom by remember { mutableFloatStateOf(1f) }
    // The zoom the pages are rendered for: it follows [zoom] once the fingers lift, so a pinch
    // scales the pages already rendered instead of rendering them on every frame.
    var renderZoom by remember { mutableFloatStateOf(1f) }
    // How far the zoomed pages are scrolled sideways, in pixels.
    var across by remember { mutableFloatStateOf(0f) }
    val list = rememberLazyListState()
    val current by remember { derivedStateOf { list.firstVisibleItemIndex + 1 } }
    val scope = rememberCoroutineScope()
    val decay = rememberSplineBasedDecay<Float>()
    // The fling or double-tap zoom under way; a new touch stops it.
    var motion by remember { mutableStateOf<Job?>(null) }
    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .clipToBounds()
            .background(MaterialTheme.colorScheme.surfaceContainer),
    ) {
        val viewport = constraints.maxWidth.toFloat()
        val gap = with(LocalDensity.current) { PAGE_GAP.toPx() }
        fun clampAcross(value: Float, zoom: Float) =
            value.coerceIn(0f, (viewport * zoom - viewport).coerceAtLeast(0f))

        // Zooms to [next] keeping the point under [focus] still, then pans by [pan]. [anchor] is
        // the list position being followed (from the top of an item); the list's own state lags a
        // frame behind, so a gesture follows its own. Returns the new anchor.
        fun zoomTo(next: Float, focus: Offset, pan: Offset, anchor: Pair<Int, Float>?): Pair<Int, Float> {
            val ratio = next / zoom
            across = clampAcross((across + focus.x) * ratio - focus.x - pan.x, next)
            val (index, offset) = anchor
                ?: (list.firstVisibleItemIndex to list.firstVisibleItemScrollOffset.toFloat())
            val moved = (offset + focus.y) * ratio - focus.y - pan.y
            list.requestScrollToItem(index, moved.roundToInt())
            zoom = next
            return index to moved
        }

        Box(
            Modifier
                .fillMaxSize()
                // Double tap zooms in about the tap, or back out to the page width.
                .pointerInput(viewport) {
                    detectTapGestures(onDoubleTap = { tap ->
                        val target = if (zoom > 1f) 1f else DOUBLE_TAP_ZOOM
                        motion?.cancel()
                        motion = scope.launch {
                            var anchor: Pair<Int, Float>? = null
                            animate(zoom, target) { value, _ -> anchor = zoomTo(value, tap, Offset.Zero, anchor) }
                            renderZoom = zoom
                        }
                    })
                }
                // Two fingers zoom about their centre and pan; the list doesn't see them. One
                // finger pans sideways here, flinging on release, and scrolls the list up and down
                // as usual.
                .pointerInput(viewport) {
                    awaitEachGesture {
                        awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                        motion?.cancel()
                        var anchor: Pair<Int, Float>? = null
                        val tracker = VelocityTracker()
                        var tracked: PointerId? = null
                        var panning = false
                        do {
                            val event = awaitPointerEvent(PointerEventPass.Initial)
                            val centroid = event.calculateCentroid(useCurrent = true)
                            if (!centroid.isSpecified) continue
                            val pan = event.calculatePan()
                            val pressed = event.changes.filter { it.pressed }
                            panning = pressed.size < 2
                            if (panning) {
                                anchor = null
                                across = clampAcross(across - pan.x, zoom)
                                pressed.firstOrNull()?.let { finger ->
                                    if (finger.id != tracked) {
                                        tracker.resetTracking()
                                        tracked = finger.id
                                    }
                                    tracker.addPosition(finger.uptimeMillis, finger.position)
                                }
                                continue
                            }
                            tracked = null
                            val next = (zoom * event.calculateZoom()).coerceIn(1f, MAX_ZOOM)
                            anchor = zoomTo(next, centroid, pan, anchor)
                            event.changes.forEach { it.consume() }
                        } while (event.changes.any { it.pressed })
                        renderZoom = zoom
                        if (panning && tracked != null && zoom > 1f) {
                            val velocity = -tracker.calculateVelocity().x
                            motion = scope.launch {
                                AnimationState(across, velocity).animateDecay(decay) {
                                    across = clampAcross(value, zoom)
                                    if (across != value) cancelAnimation()
                                }
                            }
                        }
                    }
                }
                // The pages are laid out [zoom] times as wide as the screen, shifted by [across].
                .layout { measurable, constraints ->
                    val width = (constraints.maxWidth * zoom).roundToInt()
                    val placeable = measurable.measure(constraints.copy(minWidth = width, maxWidth = width))
                    layout(constraints.maxWidth, constraints.maxHeight) {
                        placeable.place(-clampAcross(across, zoom).roundToInt(), 0)
                    }
                },
        ) {
            val renderWidth = ((viewport - 2 * gap) * renderZoom).roundToInt()
            LazyColumn(
                state = list,
                contentPadding = PaddingValues(PAGE_GAP),
                verticalArrangement = Arrangement.spacedBy(PAGE_GAP),
                modifier = Modifier.fillMaxSize(),
            ) {
                items(document.sizes.size) { index -> Page(document, index, renderWidth) }
            }
        }
        Text(
            "$current / ${document.sizes.size}",
            style = MaterialTheme.typography.labelLarge,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(16.dp)
                .background(MaterialTheme.colorScheme.surfaceContainerHighest, MaterialTheme.shapes.small)
                .padding(horizontal = 12.dp, vertical = 6.dp),
        )
    }
}

/**
 * One page, fitted to the width, rendered [renderWidth] pixels wide (at most [MAX_PAGE_PIXELS]
 * in all) when it comes into the list; the page shown is kept while a sharper one renders.
 */
@Composable
private fun Page(document: PdfPages, index: Int, renderWidth: Int) {
    val (width, height) = document.sizes[index]
    val aspect = width.toFloat() / height.coerceAtLeast(1)
    val pixels = minOf(renderWidth, sqrt(MAX_PAGE_PIXELS * aspect).toInt())
    Box(
        Modifier
            .fillMaxWidth()
            .aspectRatio(aspect)
            .background(androidx.compose.ui.graphics.Color.White),
    ) {
        val bitmap by produceState<Bitmap?>(null, document, index, pixels) {
            document.render(index, pixels)?.let { value = it }
        }
        bitmap?.let {
            Image(
                it.asImageBitmap(),
                contentDescription = "Page ${index + 1}",
                contentScale = ContentScale.FillWidth,
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}

/**
 * An open PDF: [PdfRenderer] over a copy in the cache. The renderer opens one page at a time, so
 * renders take turns.
 */
internal class PdfPages private constructor(
    private val file: File,
    private val descriptor: ParcelFileDescriptor,
    private val renderer: PdfRenderer,
    /** Each page's size, in points. */
    val sizes: List<Pair<Int, Int>>,
) {
    private val lock = Mutex()
    @Volatile private var closing = false
    private var closed = false

    /** Page [index] [widthPixels] wide, on white; null once closed. */
    suspend fun render(index: Int, widthPixels: Int): Bitmap? = lock.withLock {
        if (closing || widthPixels <= 0) return@withLock null
        val bitmap = withContext(Dispatchers.Default) {
            renderer.openPage(index).use { page ->
                val heightPixels = (widthPixels.toLong() * page.height / page.width.coerceAtLeast(1)).toInt()
                val bitmap = Bitmap.createBitmap(widthPixels, heightPixels.coerceAtLeast(1), Bitmap.Config.ARGB_8888)
                bitmap.eraseColor(Color.WHITE)
                page.render(bitmap, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                bitmap
            }
        }
        // The screen went while this page rendered: close for it.
        if (closing) closeNow()
        bitmap
    }

    /** Closes the renderer and deletes the copy, at once or after the page being rendered. */
    fun close() {
        closing = true
        if (lock.tryLock()) {
            try {
                closeNow()
            } finally {
                lock.unlock()
            }
        }
    }

    /** Only with [lock] held. */
    private fun closeNow() {
        if (closed) return
        closed = true
        renderer.close()
        descriptor.close()
        file.delete()
    }

    companion object {
        /** Writes [bytes] to [cacheDir] and opens them; throws [IOException] for other files. */
        fun open(cacheDir: File, bytes: ByteArray): PdfPages {
            val folder = File(cacheDir, "viewer").apply { mkdirs() }
            val file = File(folder, "${UUID.randomUUID()}.pdf")
            file.writeBytes(bytes)
            val descriptor = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
            try {
                val renderer = PdfRenderer(descriptor)
                val sizes = (0 until renderer.pageCount).map { index ->
                    renderer.openPage(index).use { it.width to it.height }
                }
                return PdfPages(file, descriptor, renderer, sizes)
            } catch (e: Exception) {
                descriptor.close()
                file.delete()
                throw e
            }
        }
    }
}
