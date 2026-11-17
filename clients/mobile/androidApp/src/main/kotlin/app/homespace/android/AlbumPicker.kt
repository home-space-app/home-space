package app.homespace.android

import android.content.Context
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Size
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private val COVER_SIZE = 56.dp
private const val COVER_PIXELS = 192

/**
 * A full-screen dialog to choose which albums of the phone upload, such as Camera, Screenshots or
 * WhatsApp Images. Each one goes into a folder of its own on the server.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AlbumPickerDialog(
    title: String,
    confirm: String,
    /** Albums ticked to begin with; null ticks those of the camera, under `DCIM/`. */
    initial: Set<Long>?,
    onDismiss: () -> Unit,
    onConfirm: (albums: List<MediaAlbum>) -> Unit,
) {
    val context = LocalContext.current
    val albums by produceState<List<MediaAlbum>?>(null) {
        value = withContext(Dispatchers.IO) { runCatching { mediaAlbums(context) }.getOrDefault(emptyList()) }
    }
    var picked by remember { mutableStateOf<Set<Long>?>(null) }
    val found = albums
    // Once the albums are known, so that the camera's can be ticked.
    val chosen = picked ?: found?.let { list ->
        initial ?: list.filter { it.inCameraFolder }.map { it.id }.toSet()
    }.orEmpty()

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    navigationIcon = {
                        IconButton(onClick = onDismiss) {
                            Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                        }
                    },
                )
            },
            bottomBar = {
                Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
                    Row(
                        modifier = Modifier
                            .navigationBarsPadding()
                            .fillMaxWidth()
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        val all = found.orEmpty()
                        val allChosen = all.isNotEmpty() && all.all { it.id in chosen }
                        TextButton(
                            onClick = { picked = if (allChosen) emptySet() else all.map { it.id }.toSet() },
                            enabled = all.isNotEmpty(),
                        ) { Text(if (allChosen) "Select none" else "Select all") }
                        Box(modifier = Modifier.weight(1f), contentAlignment = Alignment.CenterEnd) {
                            Button(
                                onClick = { onConfirm(all.filter { it.id in chosen }) },
                                enabled = chosen.any { id -> all.any { it.id == id } },
                            ) { Text(confirm, maxLines = 1) }
                        }
                    }
                }
            },
        ) { padding ->
            Column(modifier = Modifier.padding(padding).fillMaxSize()) {
                Text(
                    "Each album uploads into a folder of its own, inside the server folder you choose.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
                HorizontalDivider()
                when {
                    found == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator()
                    }
                    found.isEmpty() -> Text(
                        "No photos or videos found on this phone.",
                        modifier = Modifier.padding(16.dp),
                    )
                    else -> LazyColumn(modifier = Modifier.weight(1f)) {
                        items(found, key = { it.id }) { album ->
                            val on = album.id in chosen
                            ListItem(
                                leadingContent = { AlbumCover(album.cover) },
                                headlineContent = {
                                    Text(album.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                },
                                supportingContent = { Text(itemCount(album.count)) },
                                trailingContent = { Checkbox(checked = on, onCheckedChange = null) },
                                modifier = Modifier.toggleable(
                                    value = on,
                                    role = Role.Checkbox,
                                    onValueChange = { picked = if (it) chosen + album.id else chosen - album.id },
                                ),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun AlbumCover(uri: Uri) {
    val context = LocalContext.current
    val cover by produceState<Bitmap?>(null, uri) {
        value = withContext(Dispatchers.IO) { runCatching { thumbnail(context, uri) }.getOrNull() }
    }
    val shown = cover
    if (shown == null) {
        IconTile(
            R.drawable.ic_node_image,
            MaterialTheme.colorScheme.surfaceContainerHigh,
            MaterialTheme.colorScheme.onSurfaceVariant,
            size = COVER_SIZE,
        )
    } else {
        Image(
            bitmap = shown.asImageBitmap(),
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.size(COVER_SIZE).clip(TILE_SHAPE),
        )
    }
}

@Suppress("DEPRECATION") // The only way to a thumbnail before API 29.
private fun thumbnail(context: Context, uri: Uri): Bitmap? {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        return context.contentResolver.loadThumbnail(uri, Size(COVER_PIXELS, COVER_PIXELS), null)
    }
    val id = uri.lastPathSegment?.toLongOrNull() ?: return null
    return if (uri.toString().startsWith(videosUri().toString())) {
        MediaStore.Video.Thumbnails.getThumbnail(
            context.contentResolver, id, MediaStore.Video.Thumbnails.MINI_KIND, null,
        )
    } else {
        MediaStore.Images.Thumbnails.getThumbnail(
            context.contentResolver, id, MediaStore.Images.Thumbnails.MINI_KIND, null,
        )
    }
}

private fun itemCount(count: Int): String = if (count == 1) "1 item" else "$count items"
