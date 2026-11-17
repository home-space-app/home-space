package app.homespace.android

import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.DocumentsContract
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import java.io.File
import uniffi.homespace_core.SourceKind
import uniffi.homespace_core.UploadSource

private const val EXTERNAL_STORAGE = "com.android.externalstorage.documents"
private const val PRIMARY = "primary:"

/** A standard folder of the phone's shared storage, which the folder picker can open at. */
data class KnownFolder(val name: String, val description: String, val icon: Int, val directory: String) {
    /** Where the system folder picker opens, so that the user only has to confirm it. */
    val initialUri: Uri
        get() = DocumentsContract.buildDocumentUri(EXTERNAL_STORAGE, PRIMARY + directory)

    /** Whether a folder source is this folder, so that the list can say it uploads already. */
    fun isSource(source: UploadSource): Boolean =
        source.kind == SourceKind.FOLDER &&
            runCatching { DocumentsContract.getTreeDocumentId(Uri.parse(source.handle)) }.getOrNull() ==
            PRIMARY + directory
}

/**
 * The standard folders worth offering, those that exist on this phone. Photos and videos are left
 * out, since they upload by album. Android 11 and later don't let an app have the whole Download
 * folder, so it is offered only before then.
 */
private fun knownFolders(): List<KnownFolder> {
    val folders = buildList {
        add(KnownFolder("Documents", "Files saved to Documents", R.drawable.ic_node_description, Environment.DIRECTORY_DOCUMENTS))
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            add(KnownFolder("Download", "Files you've downloaded", R.drawable.ic_download, Environment.DIRECTORY_DOWNLOADS))
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            add(KnownFolder("Recordings", "Voice and call recordings", R.drawable.ic_mic, Environment.DIRECTORY_RECORDINGS))
        }
        add(KnownFolder("Music", "Songs and other audio", R.drawable.ic_music_note, Environment.DIRECTORY_MUSIC))
    }
    @Suppress("DEPRECATION") // Only to tell whether the folder is there; reading goes by the picker.
    val root = Environment.getExternalStorageDirectory()
    return folders.filter { runCatching { File(root, it.directory).isDirectory }.getOrDefault(true) }
}

/**
 * What should upload, as a full-screen list: photos and videos by album first, then the standard
 * folders, then any other folder. Saves the user from finding their way around the file system.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UploadChoicesDialog(
    sources: List<UploadSource>,
    onDismiss: () -> Unit,
    onPhotos: () -> Unit,
    onKnownFolder: (KnownFolder) -> Unit,
    onOtherFolder: () -> Unit,
) {
    val folders = remember { knownFolders() }
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text("What should upload?", maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    navigationIcon = {
                        IconButton(onClick = onDismiss) {
                            Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                        }
                    },
                )
            },
        ) { padding ->
            Column(modifier = Modifier.padding(padding).fillMaxSize().verticalScroll(rememberScrollState())) {
                Choice(
                    icon = R.drawable.ic_node_image,
                    name = "Photos and videos",
                    description = "Camera, screenshots and other albums, such as WhatsApp Images",
                    onClick = onPhotos,
                )
                SectionHeader("Folders")
                folders.forEach { folder ->
                    val uploading = sources.firstOrNull(folder::isSource)
                    Choice(
                        icon = folder.icon,
                        name = folder.name,
                        description = uploading?.let { "Uploads to ${it.targetLabel} already" } ?: folder.description,
                        onClick = { onKnownFolder(folder) },
                    )
                }
                HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
                Choice(
                    icon = R.drawable.ic_folder_open,
                    name = "Another folder",
                    description = "Choose any folder on this phone",
                    onClick = onOtherFolder,
                )
                Text(
                    "For a folder, Android asks you to confirm it: tap “Use this folder”, then “Allow”.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(16.dp),
                )
            }
        }
    }
}

@Composable
private fun Choice(icon: Int, name: String, description: String, onClick: () -> Unit) {
    ListItem(
        leadingContent = {
            IconTile(icon, MaterialTheme.colorScheme.secondaryContainer, MaterialTheme.colorScheme.onSecondaryContainer)
        },
        headlineContent = { Text(name) },
        supportingContent = { Text(description) },
        trailingContent = { Chevron() },
        modifier = Modifier.clickable(onClick = onClick),
    )
}
