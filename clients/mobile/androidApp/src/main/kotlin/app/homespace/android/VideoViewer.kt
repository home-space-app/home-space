package app.homespace.android

import android.os.SystemClock
import androidx.annotation.OptIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.homespace_core.CoreClient
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.Entry

private const val CANNOT_PLAY = "This phone can't play this video. Download it to open it in another app."
/** A failure this soon after a URL starts is the file, not an expired stream ticket. */
private const val RETRY_AFTER_MS = 60_000L

/** The video viewer of one of the user's own files; the screen gets the stream URL. */
class VideoViewerState(override val entry: Entry) : OpenViewer {
    override fun load(scope: CoroutineScope, client: CoreClient, onSessionEnded: () -> Unit) = Unit
}

@Composable
internal fun VideoViewerScreen(session: Session.SignedIn, model: SessionViewModel, state: VideoViewerState) {
    VideoScreen(
        key = state.entry.id,
        title = state.entry.name,
        onBack = model::closeViewer,
        streamUrl = { session.client.streamUrl(state.entry.id) },
        onSessionEnded = model::onSessionEnded,
    )
}

/**
 * A video, streamed from the server and played with Media3's player. The URL carries a stream
 * ticket that lasts an hour: when playback fails after a while, the screen gets a new URL and
 * resumes where it was; when it fails right after starting, the phone cannot play the file.
 */
@OptIn(UnstableApi::class)
@Composable
internal fun VideoScreen(
    key: Any,
    title: String,
    onBack: () -> Unit,
    streamUrl: suspend () -> String,
    onSessionEnded: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // Where playback was, kept across rotation.
    var position by rememberSaveable(key) { mutableLongStateOf(0L) }
    var error by remember(key) { mutableStateOf<String?>(null) }
    var started by remember(key) { mutableStateOf(false) }
    val player = remember(key) { ExoPlayer.Builder(context).build() }

    fun play(resumeAt: Long) {
        scope.launch {
            try {
                val url = streamUrl()
                player.setMediaItem(MediaItem.fromUri(url), resumeAt)
                player.prepare()
                player.play()
                started = true
            } catch (_: CoreException.Unauthorized) {
                onSessionEnded()
            } catch (e: CoreException) {
                error = e.fileMessage()
            }
        }
    }

    DisposableEffect(player) {
        var lastStart = SystemClock.elapsedRealtime()
        val listener = object : Player.Listener {
            override fun onPlayerError(failure: PlaybackException) {
                val now = SystemClock.elapsedRealtime()
                if (now - lastStart < RETRY_AFTER_MS) {
                    error = CANNOT_PLAY
                } else {
                    lastStart = now
                    play(player.currentPosition)
                }
            }
        }
        player.addListener(listener)
        onDispose {
            position = player.currentPosition
            player.removeListener(listener)
            player.release()
        }
    }
    LaunchedEffect(player) { play(position) }

    SubPageScaffold(title = title, onBack = onBack) { padding ->
        Box(Modifier.padding(padding).fillMaxSize().background(Color.Black)) {
            val message = error
            when {
                message != null -> Text(
                    message,
                    color = Color.White,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.align(Alignment.Center).padding(24.dp),
                )
                !started -> CircularProgressIndicator(Modifier.align(Alignment.Center))
                else -> AndroidView(
                    factory = { PlayerView(it).apply { this.player = player } },
                    modifier = Modifier.fillMaxSize(),
                )
            }
        }
    }
}
