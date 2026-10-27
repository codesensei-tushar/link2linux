package dev.link2linux.android.core.designsystem.component

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import dev.link2linux.android.core.designsystem.R
import dev.link2linux.android.core.model.Feature

@get:StringRes
val Feature.titleRes: Int
    get() = when (this) {
        Feature.PAIRING -> R.string.feature_title_pairing
        Feature.NOTIFICATIONS -> R.string.feature_title_notifications
        Feature.CALLS -> R.string.feature_title_calls
        Feature.PHOTOS -> R.string.feature_title_photos
        Feature.FILES -> R.string.feature_title_files
        Feature.CLIPBOARD -> R.string.feature_title_clipboard
        Feature.SCREEN -> R.string.feature_title_screen
        Feature.STATUS -> R.string.feature_title_status
        Feature.RING -> R.string.feature_title_ring
    }

@get:StringRes
val Feature.summaryRes: Int
    get() = when (this) {
        Feature.PAIRING -> R.string.feature_summary_pairing
        Feature.NOTIFICATIONS -> R.string.feature_summary_notifications
        Feature.CALLS -> R.string.feature_summary_calls
        Feature.PHOTOS -> R.string.feature_summary_photos
        Feature.FILES -> R.string.feature_summary_files
        Feature.CLIPBOARD -> R.string.feature_summary_clipboard
        Feature.SCREEN -> R.string.feature_summary_screen
        Feature.STATUS -> R.string.feature_summary_status
        Feature.RING -> R.string.feature_summary_ring
    }

@get:DrawableRes
val Feature.iconRes: Int
    get() = when (this) {
        Feature.PAIRING -> R.drawable.ic_link
        Feature.NOTIFICATIONS -> R.drawable.ic_notifications
        Feature.CALLS -> R.drawable.ic_call
        Feature.PHOTOS -> R.drawable.ic_photo_library
        Feature.FILES -> R.drawable.ic_folder
        Feature.CLIPBOARD -> R.drawable.ic_content_paste
        Feature.SCREEN -> R.drawable.ic_screen_share
        Feature.STATUS -> R.drawable.ic_battery
        Feature.RING -> R.drawable.ic_ring
    }

/** One row in a card listing features: icon tile, name, one-line summary. */
@Composable
fun FeatureListItem(
    feature: Feature,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    L2lListItem(
        headlineContent = { Text(stringResource(feature.titleRes)) },
        supportingContent = { Text(stringResource(feature.summaryRes)) },
        leadingContent = { IconTile(painterResource(feature.iconRes)) },
        trailingContent = { Icon(painterResource(R.drawable.ic_chevron_right), contentDescription = null) },
        modifier = modifier.clickable(onClick = onClick),
    )
}

/** Screen shown for a feature that is not built yet. */
@Composable
fun FeaturePlaceholderScreen(
    feature: Feature,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(feature.titleRes), onBack = onBack) },
    ) { padding ->
        Box(
            modifier = Modifier
                .padding(padding)
                .fillMaxSize(),
            contentAlignment = Alignment.Center,
        ) {
            EmptyState(
                icon = painterResource(feature.iconRes),
                title = stringResource(R.string.feature_placeholder_title),
                body = stringResource(feature.summaryRes) + "\n\n" +
                    stringResource(R.string.feature_placeholder_body),
            )
        }
    }
}
