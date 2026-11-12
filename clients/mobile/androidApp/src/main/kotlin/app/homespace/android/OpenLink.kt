package app.homespace.android

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import uniffi.homespace_core.TransferLink

/** A link of a received transfer, waiting for the user to say it may open. */
data class PendingLink(val transferId: String, val link: TransferLink)

/** Hands a link to whichever app handles it: a browser for a web page. False when none does. */
fun openLink(context: Context, uri: String): Boolean {
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(uri))
        .addCategory(Intent.CATEGORY_BROWSABLE)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    return try {
        context.startActivity(intent)
        true
    } catch (_: ActivityNotFoundException) {
        false
    } catch (_: SecurityException) {
        // An app that exports the activity but guards it with a permission.
        false
    }
}

/**
 * Shows a received link before it opens, so that the user can refuse one that looks wrong: where it
 * goes, with the host as the system will see it (a look-alike name shows as `xn--…`), and the whole
 * link.
 */
@Composable
fun OpenLinkDialog(pending: PendingLink, onOpen: () -> Unit, onDismiss: () -> Unit) {
    val link = pending.link
    val where = when {
        link.web && link.host != null -> link.host
        link.host != null -> "${link.host} in the app for “${link.scheme}:” links"
        else -> "the app for “${link.scheme}:” links"
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(painterResource(R.drawable.ic_open_in_new), contentDescription = null) },
        title = { Text("Open this link?") },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(12.dp),
                // A long link scrolls rather than pushing the buttons off the screen.
                modifier = Modifier.heightIn(max = 320.dp).verticalScroll(rememberScrollState()),
            ) {
                Text(
                    if (link.web) "It opens in your browser at" else "It opens",
                    style = MaterialTheme.typography.bodyMedium,
                )
                Text(where.orEmpty(), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                SelectionContainer {
                    Text(
                        link.uri,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (link.others > 0u) {
                    Text(
                        "Only the first opens; the list has ${link.others} more. Download it to see them all.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Text(
                    "Open it only if you expected it and it looks right.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        },
        confirmButton = { TextButton(onClick = onOpen) { Text("Open") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
