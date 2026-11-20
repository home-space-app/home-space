package app.homespace.android

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.homespace_core.EntryIcon

internal val TILE_SIZE = 40.dp
internal val TILE_SHAPE = RoundedCornerShape(12.dp)
internal val ROW_MIN_HEIGHT = 72.dp
/** Where a row's text starts: the padding, its tile, and the gap. */
internal val ROW_TEXT_START = 72.dp
internal const val PERCENT = 100

private const val TABULAR_NUMERALS = "tnum"
private val CHIP_SHAPE = RoundedCornerShape(8.dp)
private val CHIP_ICON_SIZE = 16.dp

/** Sizes and dates line up from row to row. */
internal fun TextStyle.tabular(): TextStyle = copy(fontFeatureSettings = TABULAR_NUMERALS)

/** A server's address without the scheme, which says little to a reader. */
internal fun serverLabel(address: String): String = address.substringAfter("://")

@Composable
internal fun IconTile(
    icon: Int,
    container: Color,
    content: Color,
    modifier: Modifier = Modifier,
    size: Dp = TILE_SIZE,
) {
    Box(
        modifier = modifier.size(size).background(container, TILE_SHAPE),
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = content)
    }
}

/** The same icons as the web client: only folders are coloured. */
@Composable
internal fun NodeTile(icon: EntryIcon, modifier: Modifier = Modifier, size: Dp = TILE_SIZE) {
    val colors = MaterialTheme.colorScheme
    val drawable = when (icon) {
        EntryIcon.FOLDER -> R.drawable.ic_folder_filled
        EntryIcon.IMAGE -> R.drawable.ic_node_image
        // No movie icon is bundled yet; video shares the icon of other files.
        EntryIcon.VIDEO -> R.drawable.ic_node_draft
        EntryIcon.DOCUMENT -> R.drawable.ic_node_description
        EntryIcon.OTHER -> R.drawable.ic_node_draft
    }
    if (icon == EntryIcon.FOLDER) {
        IconTile(drawable, colors.primaryContainer, colors.onPrimaryContainer, modifier, size)
    } else {
        IconTile(drawable, colors.surfaceContainerHigh, colors.onSurfaceVariant, modifier, size)
    }
}

/** A list item's plain icon, in the width of a tile so that the text lines up with tiled rows. */
@Composable
internal fun LeadingIcon(icon: Int) {
    Box(modifier = Modifier.size(TILE_SIZE), contentAlignment = Alignment.Center) {
        Icon(
            painterResource(icon),
            contentDescription = null,
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
internal fun Chevron() {
    Icon(
        painterResource(R.drawable.ic_chevron_right),
        contentDescription = null,
        tint = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
internal fun SectionHeader(title: String, modifier: Modifier = Modifier) {
    Text(
        title,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = modifier.padding(start = 16.dp, top = 24.dp, end = 16.dp, bottom = 8.dp),
    )
}

/** The user's initial; there are no pictures to show. */
@Composable
internal fun Avatar(name: String, size: Dp, style: TextStyle) {
    Box(
        modifier = Modifier.size(size).background(MaterialTheme.colorScheme.primaryContainer, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            name.take(1).uppercase(),
            style = style,
            color = MaterialTheme.colorScheme.onPrimaryContainer,
        )
    }
}

internal enum class ChipTone { SUCCESS, ERROR, NEUTRAL }

/** A state, not a control: it doesn't react to a tap. */
@Composable
internal fun StatusChip(label: String, tone: ChipTone, icon: Int? = null) {
    val (container, content) = when (tone) {
        ChipTone.SUCCESS -> LocalExtendedColors.current.let { it.successContainer to it.onSuccessContainer }
        ChipTone.ERROR -> MaterialTheme.colorScheme.errorContainer to MaterialTheme.colorScheme.onErrorContainer
        ChipTone.NEUTRAL -> MaterialTheme.colorScheme.secondaryContainer to MaterialTheme.colorScheme.onSecondaryContainer
    }
    Surface(shape = CHIP_SHAPE, color = container, contentColor = content) {
        Row(
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            icon?.let {
                Icon(painterResource(it), contentDescription = null, modifier = Modifier.size(CHIP_ICON_SIZE))
            }
            Text(label, style = MaterialTheme.typography.labelMedium, maxLines = 1)
        }
    }
}

/** How far something has come; going back and forth while it isn't known. */
@Composable
internal fun ProgressLine(progress: Float?, modifier: Modifier = Modifier) {
    if (progress != null) {
        LinearProgressIndicator(progress = { progress }, modifier = modifier)
    } else {
        LinearProgressIndicator(modifier = modifier)
    }
}

/** Where a folder is; tapping a name goes there. The last one is the folder itself. */
@Composable
internal fun Breadcrumbs(names: List<String>, onOpen: (Int) -> Unit, modifier: Modifier = Modifier) {
    val state = rememberLazyListState()
    // The end is the part that matters when a deep path doesn't fit.
    LaunchedEffect(names.size) { state.scrollToItem(names.lastIndex.coerceAtLeast(0)) }
    LazyRow(
        state = state,
        modifier = modifier,
        contentPadding = PaddingValues(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        itemsIndexed(names) { index, name ->
            val last = index == names.lastIndex
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    modifier = Modifier
                        .heightIn(min = 40.dp)
                        .clickable(enabled = !last) { onOpen(index) }
                        .padding(horizontal = 8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        name,
                        style = MaterialTheme.typography.labelLarge,
                        color = if (last) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (!last) Chevron()
            }
        }
    }
}
