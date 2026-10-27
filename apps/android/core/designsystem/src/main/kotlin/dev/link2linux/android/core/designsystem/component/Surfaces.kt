package dev.link2linux.android.core.designsystem.component

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ProvideTextStyle
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.painter.Painter
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.link2linux.android.core.designsystem.theme.L2lTheme

// The desktop's textures: white cards with a hairline border and a faint shadow on a light gray
// page, titled groups, and soft-colored status pills. Screens are lists of these, with 16 dp side
// margins, rather than full-width rows.

/** A card: the desktop's surface color, a hairline border, 10 dp corners and a faint shadow. */
@Composable
fun L2lCard(
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    Surface(
        modifier = modifier,
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.surfaceContainerLowest,
        contentColor = MaterialTheme.colorScheme.onSurface,
        border = BorderStroke(1.dp, L2lTheme.colors.border),
        shadowElevation = 1.dp,
    ) {
        Column(content = content)
    }
}

/** The title above a group of cards, like the desktop's "Paired phones". */
@Composable
fun L2lSectionHeader(
    text: String,
    modifier: Modifier = Modifier,
) {
    Text(
        text = text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.onSurface,
        modifier = modifier.padding(start = 20.dp, top = 20.dp, end = 20.dp, bottom = 8.dp),
    )
}

/**
 * A group of settings or actions in one card, with an optional title above it. Separate the rows
 * with [L2lDivider]. For a long or growing list, give each entry its own [L2lCard] instead.
 */
@Composable
fun L2lSection(
    modifier: Modifier = Modifier,
    title: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(modifier.fillMaxWidth()) {
        if (title != null) L2lSectionHeader(title)
        L2lCard(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = if (title == null) 8.dp else 0.dp),
            content = content,
        )
    }
}

/** The hairline between rows of a card. */
@Composable
fun L2lDivider(modifier: Modifier = Modifier) {
    HorizontalDivider(modifier = modifier, color = L2lTheme.colors.border)
}

/**
 * A row inside a card: a bold label over a muted hint, like the desktop's settings. The row is
 * transparent, so it takes the card's color.
 */
@Composable
fun L2lListItem(
    headlineContent: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    overlineContent: (@Composable () -> Unit)? = null,
    supportingContent: (@Composable () -> Unit)? = null,
    leadingContent: (@Composable () -> Unit)? = null,
    trailingContent: (@Composable () -> Unit)? = null,
) {
    ListItem(
        headlineContent = {
            ProvideTextStyle(MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.Bold)) {
                headlineContent()
            }
        },
        modifier = modifier,
        overlineContent = overlineContent,
        supportingContent = supportingContent,
        leadingContent = leadingContent,
        trailingContent = trailingContent,
        colors = ListItemDefaults.colors(
            containerColor = Color.Transparent,
            headlineColor = MaterialTheme.colorScheme.onSurface,
            supportingColor = MaterialTheme.colorScheme.onSurfaceVariant,
            leadingIconColor = MaterialTheme.colorScheme.onSurfaceVariant,
            trailingIconColor = L2lTheme.colors.textSubtle,
        ),
    )
}

/** An icon on a soft accent tile, the desktop's empty-state mark at list size. */
@Composable
fun IconTile(
    icon: Painter,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .size(40.dp)
            .clip(MaterialTheme.shapes.medium)
            .background(MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(22.dp))
    }
}

/** The desktop's status colors. */
enum class Tone { Neutral, Accent, Success, Warning, Danger }

/** A soft pill with a dot, the desktop's badge: "Connected", "Not connected", ... */
@Composable
fun StatusBadge(
    text: String,
    tone: Tone,
    modifier: Modifier = Modifier,
) {
    val colors = L2lTheme.colors
    val (content, container, dot) = when (tone) {
        Tone.Neutral -> Triple(MaterialTheme.colorScheme.onSurfaceVariant, MaterialTheme.colorScheme.surfaceVariant, colors.textSubtle)
        Tone.Accent -> Triple(MaterialTheme.colorScheme.primary, colors.accentSoft, MaterialTheme.colorScheme.primary)
        Tone.Success -> Triple(colors.success, colors.successSoft, colors.success)
        Tone.Warning -> Triple(colors.warning, colors.warningSoft, colors.warning)
        Tone.Danger -> Triple(colors.danger, colors.dangerSoft, colors.danger)
    }
    Row(
        modifier = modifier
            .clip(CircleShape)
            .background(container)
            .padding(horizontal = 10.dp, vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier
                .size(6.dp)
                .background(dot, CircleShape),
        )
        Text(text, style = MaterialTheme.typography.labelMedium, color = content)
    }
}

/**
 * A tinted note with an icon, the desktop's callout: [Tone.Accent] for information and
 * [Tone.Warning] for something the user should fix.
 */
@Composable
fun L2lCallout(
    text: String,
    modifier: Modifier = Modifier,
    tone: Tone = Tone.Accent,
    icon: Painter? = null,
) {
    val colors = L2lTheme.colors
    val (soft, strong) = when (tone) {
        Tone.Warning -> colors.warningSoft to colors.warning
        Tone.Danger -> colors.dangerSoft to colors.danger
        Tone.Success -> colors.successSoft to colors.success
        Tone.Neutral -> MaterialTheme.colorScheme.surfaceVariant to colors.textSubtle
        Tone.Accent -> colors.accentSoft to MaterialTheme.colorScheme.primary
    }
    Surface(
        modifier = modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.medium,
        color = soft.copy(alpha = 0.6f).compositeOver(MaterialTheme.colorScheme.background),
        contentColor = MaterialTheme.colorScheme.onSurface,
        border = BorderStroke(1.dp, soft),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (icon != null) {
                Icon(icon, contentDescription = null, tint = strong, modifier = Modifier.size(20.dp))
            }
            Text(text, style = MaterialTheme.typography.bodyMedium)
        }
    }
}
