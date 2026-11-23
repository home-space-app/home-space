package app.homespace.android

import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import uniffi.homespace_core.UpdateOffer
import uniffi.homespace_core.VersionCheck
import uniffi.homespace_core.VersionTone
import uniffi.homespace_core.newVersion
import uniffi.homespace_core.updateLabels

private val DOT_SIZE = 8.dp
private val SMALL_DOT_SIZE = 6.dp
private val CARD_SHAPE = RoundedCornerShape(12.dp)

/** The "New version available" choices kept on this phone, never synced. */
object UpdatePrefs {
    private const val PREFS = "update"
    private const val CHECK = "check"
    private const val SKIPPED = "skipped_version"
    private const val BANNER_DISMISSED = "banner_dismissed_for_server"

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** Whether to ask GitHub for the latest release when the app starts; on unless turned off. */
    fun checks(context: Context): Boolean = prefs(context).getBoolean(CHECK, true)

    fun setChecks(context: Context, on: Boolean) {
        prefs(context).edit().putBoolean(CHECK, on).apply()
        if (!on) UpdateCheck.offer = null
    }

    fun skipped(context: Context): String? = prefs(context).getString(SKIPPED, null)

    /** Skips `version`: only a newer release is offered from now on. */
    fun skip(context: Context, version: String) {
        prefs(context).edit().putString(SKIPPED, version).apply()
        UpdateCheck.offer = null
    }

    /** The server version the red banner was dismissed for; it shows again for another one. */
    fun bannerDismissedFor(context: Context): String? = prefs(context).getString(BANNER_DISMISSED, null)

    fun dismissBanner(context: Context, serverVersion: String?) {
        prefs(context).edit().putString(BANNER_DISMISSED, serverVersion.orEmpty()).apply()
    }
}

/** The release the toast offers, read once each time the app process starts. */
object UpdateCheck {
    var offer by mutableStateOf<UpdateOffer?>(null)
    private var checked = false

    /** Asks for the latest release unless already asked or turned off. A failure shows nothing. */
    suspend fun run(context: Context) {
        if (checked || !UpdatePrefs.checks(context)) return
        checked = true
        offer = newVersion(UpdatePrefs.skipped(context), null)
    }
}

/** Whether the red banner shows for `check` on this phone. */
fun showsVersionBanner(context: Context, check: VersionCheck?): Boolean =
    check?.banner != null && UpdatePrefs.bannerDismissedFor(context) != check.serverVersion.orEmpty()

@Composable
internal fun versionColor(tone: VersionTone): Color = when (tone) {
    VersionTone.SAME -> LocalExtendedColors.current.success
    VersionTone.WARNING -> LocalExtendedColors.current.warning
    VersionTone.ERROR -> MaterialTheme.colorScheme.error
}

@Composable
internal fun VersionDot(tone: VersionTone, small: Boolean = false) {
    Box(
        Modifier
            .size(if (small) SMALL_DOT_SIZE else DOT_SIZE)
            .background(versionColor(tone), CircleShape),
    )
}

/** The version chip under the server on the profile; opens [VersionSheet]. */
@Composable
internal fun VersionChip(check: VersionCheck, onClick: () -> Unit) {
    Surface(
        shape = RoundedCornerShape(8.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.clickable(onClick = onClick),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            VersionDot(check.tone)
            Text(check.chip, style = MaterialTheme.typography.labelLarge, maxLines = 1)
        }
    }
}

/** Both versions, the state and what to do about it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun VersionSheet(check: VersionCheck, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                VersionDot(check.tone)
                Text(check.title, style = MaterialTheme.typography.titleLarge, color = versionColor(check.tone))
            }
            VersionFact("This app", check.appVersion)
            VersionFact("Server", check.serverVersion ?: "unknown")
            Text(check.hint, style = MaterialTheme.typography.bodyMedium)
            if (check.missingFeatures.isNotEmpty()) {
                Surface(shape = CARD_SHAPE, color = MaterialTheme.colorScheme.errorContainer) {
                    Column(modifier = Modifier.fillMaxWidth().padding(12.dp)) {
                        Text("Won't work until the server is updated", style = MaterialTheme.typography.labelLarge)
                        check.missingFeatures.forEach { Text("• $it", style = MaterialTheme.typography.bodyMedium) }
                    }
                }
            }
        }
    }
}

@Composable
private fun VersionFact(label: String, value: String) {
    Row {
        Text(label, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
        Text(value, style = MaterialTheme.typography.bodyMedium.tabular())
    }
}

/** The red banner at the top of Files, until dismissed for this server version. */
@Composable
internal fun VersionBanner(check: VersionCheck, onDetails: () -> Unit, onDismiss: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 16.dp, top = 8.dp, bottom = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(painterResource(R.drawable.ic_error), contentDescription = null, tint = MaterialTheme.colorScheme.error)
            Column(modifier = Modifier.weight(1f)) {
                Text(check.title, style = MaterialTheme.typography.labelLarge)
                Text(check.banner.orEmpty(), style = MaterialTheme.typography.bodySmall)
            }
            TextButton(onClick = onDetails) { Text("Details") }
            IconButton(onClick = onDismiss) {
                Icon(painterResource(R.drawable.ic_close), contentDescription = "Dismiss")
            }
        }
    }
}

/** The "New version available" card, over the screen above the navigation bar. */
@Composable
internal fun UpdateCard(
    offer: UpdateOffer,
    onClose: () -> Unit,
    onSkip: () -> Unit,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val labels = updateLabels()
    Surface(
        shape = CARD_SHAPE,
        color = MaterialTheme.colorScheme.inverseSurface,
        contentColor = MaterialTheme.colorScheme.inverseOnSurface,
        shadowElevation = 6.dp,
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(start = 16.dp, top = 6.dp, end = 4.dp, bottom = 4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    painterResource(R.drawable.ic_info),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.inversePrimary,
                    modifier = Modifier.size(20.dp),
                )
                Spacer(Modifier.size(10.dp))
                Text(labels.title, style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
                IconButton(onClick = onClose) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Close")
                }
            }
            Text(offer.message, style = MaterialTheme.typography.bodyMedium)
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = onSkip) {
                    Text(labels.skip, color = MaterialTheme.colorScheme.inversePrimary)
                }
                TextButton(onClick = onOpen) {
                    Text(labels.openWeb, color = MaterialTheme.colorScheme.inversePrimary)
                }
            }
        }
    }
}
