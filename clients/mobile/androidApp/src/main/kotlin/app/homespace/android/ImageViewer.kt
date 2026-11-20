package app.homespace.android

import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.os.Build
import androidx.compose.animation.core.animate
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.geometry.lerp
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.util.lerp
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import java.io.IOException
import java.nio.ByteBuffer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry
import uniffi.homespace_core.ViewerKind
import uniffi.homespace_core.addViewerTypes

private const val NOT_DECODED = "This image cannot be shown on this phone."
private val PAGE_PADDING = 16.dp
private const val MAX_ZOOM = 5f
private const val DOUBLE_TAP_ZOOM = 2.5f

/**
 * Tells the Rust viewer registry which image types this phone decodes beyond the ones every
 * client shows. Call it once when the app starts.
 */
fun registerPlatformImageTypes() {
    val types = buildList {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) addAll(listOf("image/heic", "image/heif"))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) add("image/avif")
    }
    if (types.isNotEmpty()) addViewerTypes(ViewerKind.IMAGE, types)
}

/** The image viewer: the pictures of the folder, one per page, starting at [entry]. */
class ImageViewerState(override val entry: Entry) : OpenViewer {
    /** The pictures to step through; just [entry] until the folder is listed. */
    var entries by mutableStateOf(listOf(entry))
        private set
    var start by mutableStateOf(0)
        private set
    var error by mutableStateOf<String?>(null)
        private set

    override fun load(scope: CoroutineScope, client: CoreClient, onSessionEnded: () -> Unit) {
        error = null
        scope.launch {
            try {
                val gallery = client.gallery(entry.id)
                val index = gallery.index
                if (index != null && gallery.entries.isNotEmpty()) {
                    entries = gallery.entries
                    start = index.toInt()
                }
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
            }
        }
    }
}

@Composable
internal fun ImageViewerScreen(session: Session.SignedIn, model: SessionViewModel, state: ImageViewerState) {
    val entries = state.entries
    // A new pager once the folder is listed, so it starts on the image that was tapped.
    val pager = remember(entries) { PagerState(currentPage = state.start) { entries.size } }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    val current = entries.getOrNull(pager.currentPage) ?: state.entry
    val step = { offset: Int ->
        val target = pager.currentPage + offset
        if (target in entries.indices) scope.launch { pager.animateScrollToPage(target) }
    }
    SubPageScaffold(
        title = current.name,
        onBack = model::closeViewer,
        actions = {
            if (entries.size > 1) {
                IconButton(onClick = { step(-1) }, enabled = pager.currentPage > 0) {
                    Icon(painterResource(R.drawable.ic_chevron_left), contentDescription = "Previous image")
                }
                Text(
                    "${pager.currentPage + 1} / ${entries.size}",
                    style = MaterialTheme.typography.labelLarge,
                )
                IconButton(onClick = { step(1) }, enabled = pager.currentPage < entries.lastIndex) {
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
            state.error?.let { message ->
                Text(
                    message,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(horizontal = PAGE_PADDING, vertical = 8.dp),
                )
            }
            HorizontalPager(state = pager, key = { entries[it].id }, modifier = Modifier.fillMaxSize()) { page ->
                val entry = entries[page]
                ImagePage(
                    key = entry.id to entry.sha256,
                    name = entry.name,
                    load = { session.client.readImage(entry.id).bytes },
                    onSessionEnded = model::onSessionEnded,
                )
            }
        }
    }
    LaunchedEffect(Unit) { focus.requestFocus() }
}

private sealed interface Picture {
    data object Loading : Picture
    data class Shown(val bitmap: ImageBitmap) : Picture
    data class Failed(val message: String) : Picture
}

/** One picture, read with `load` whenever `key` changes, fitted to the page. */
@Composable
internal fun ImagePage(key: Any, name: String, load: suspend () -> ByteArray, onSessionEnded: () -> Unit) {
    val metrics = LocalContext.current.resources.displayMetrics
    val maxSide = maxOf(metrics.widthPixels, metrics.heightPixels)
    val picture by produceState<Picture>(Picture.Loading, key) {
        value = try {
            decode(load(), maxSide)?.let { Picture.Shown(it) } ?: Picture.Failed(NOT_DECODED)
        } catch (_: CoreException.Unauthorized) {
            onSessionEnded()
            Picture.Loading
        } catch (e: CoreException) {
            Picture.Failed(e.fileMessage())
        }
    }
    Box(Modifier.fillMaxSize().clipToBounds().padding(PAGE_PADDING), Alignment.Center) {
        when (val shown = picture) {
            Picture.Loading -> CircularProgressIndicator()
            is Picture.Shown -> ZoomableImage(shown.bitmap, name)
            is Picture.Failed -> Text(
                shown.message,
                textAlign = TextAlign.Center,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

/**
 * A picture fitted to the page, with pinch zoom and double-tap zoom, as in Photos. While it is
 * not zoomed, one-finger swipes go to the pager. While zoomed, they pan the picture, and once
 * the picture's edge is reached they go to the pager again, so the next picture is one swipe away.
 */
@Composable
private fun ZoomableImage(bitmap: ImageBitmap, name: String) {
    var scale by remember(bitmap) { mutableFloatStateOf(1f) }
    var offset by remember(bitmap) { mutableStateOf(Offset.Zero) }
    val scope = rememberCoroutineScope()
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val box = Size(constraints.maxWidth.toFloat(), constraints.maxHeight.toFloat())
        val aspect = bitmap.width.toFloat() / bitmap.height.coerceAtLeast(1)
        val fittedWidth = minOf(box.width, box.height * aspect)
        val fitted = Size(fittedWidth, fittedWidth / aspect)

        // Keeps the picture covering the page where it is larger than it, centred where it is not.
        fun clamp(value: Offset, zoom: Float): Offset {
            val maxX = ((fitted.width * zoom - box.width) / 2).coerceAtLeast(0f)
            val maxY = ((fitted.height * zoom - box.height) / 2).coerceAtLeast(0f)
            return Offset(value.x.coerceIn(-maxX, maxX), value.y.coerceIn(-maxY, maxY))
        }

        // The offset that keeps the point under `focus` (from the centre) still while zooming.
        fun zoomed(focus: Offset, from: Float, to: Float, start: Offset): Offset =
            clamp(focus - (focus - start) * (to / from), to)

        val center = Offset(box.width / 2, box.height / 2)
        Image(
            bitmap = bitmap,
            contentDescription = name,
            contentScale = ContentScale.Fit,
            modifier = Modifier
                .fillMaxSize()
                .pointerInput(bitmap) {
                    detectTapGestures(onDoubleTap = { tap ->
                        val fromScale = scale
                        val fromOffset = offset
                        val toScale = if (scale > 1f) 1f else DOUBLE_TAP_ZOOM
                        val toOffset = if (toScale == 1f) Offset.Zero
                        else zoomed(tap - center, 1f, toScale, Offset.Zero)
                        scope.launch {
                            animate(0f, 1f) { progress, _ ->
                                scale = lerp(fromScale, toScale, progress)
                                offset = lerp(fromOffset, toOffset, progress)
                            }
                        }
                    })
                }
                .pointerInput(bitmap) {
                    awaitEachGesture {
                        awaitFirstDown(requireUnconsumed = false)
                        do {
                            val event = awaitPointerEvent()
                            val fingers = event.changes.count { it.pressed }
                            if (fingers < 2 && scale <= 1f) continue
                            val zoom = (scale * event.calculateZoom()).coerceIn(1f, MAX_ZOOM)
                            val focus = event.calculateCentroid(useCurrent = true) - center
                            val wanted = focus - (focus - offset) * (zoom / scale) + event.calculatePan()
                            val next = clamp(wanted, zoom)
                            // A one-finger swipe past the edge is left to the pager.
                            val atEdge = fingers < 2 && next == offset && wanted != offset
                            scale = zoom
                            offset = next
                            if (!atEdge) event.changes.forEach { if (it.positionChanged()) it.consume() }
                        } while (event.changes.any { it.pressed })
                    }
                }
                .graphicsLayer {
                    scaleX = scale
                    scaleY = scale
                    translationX = offset.x
                    translationY = offset.y
                },
        )
    }
}

/** Decodes `bytes` off the main thread, scaled down by powers of two to about `maxSide` pixels. */
private suspend fun decode(bytes: ByteArray, maxSide: Int): ImageBitmap? = withContext(Dispatchers.Default) {
    try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            val source = ImageDecoder.createSource(ByteBuffer.wrap(bytes))
            ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
                decoder.setTargetSampleSize(sampleSize(maxOf(info.size.width, info.size.height), maxSide))
            }.asImageBitmap()
        } else {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            val options = BitmapFactory.Options().apply {
                inSampleSize = sampleSize(maxOf(bounds.outWidth, bounds.outHeight), maxSide)
            }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.asImageBitmap()
        }
    } catch (_: IOException) {
        null
    } catch (_: IllegalArgumentException) {
        null
    }
}

/** The largest power of two that keeps an image of `side` pixels at least `maxSide` pixels. */
internal fun sampleSize(side: Int, maxSide: Int): Int {
    var sample = 1
    while (maxSide > 0 && side / (sample * 2) >= maxSide) sample *= 2
    return sample
}
