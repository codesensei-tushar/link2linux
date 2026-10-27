package dev.link2linux.android.core.designsystem.component

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
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
    }

/** One row in a list of features: icon, name, one-line summary. */
@Composable
fun FeatureListItem(
    feature: Feature,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    ListItem(
        headlineContent = { Text(stringResource(feature.titleRes)) },
        supportingContent = { Text(stringResource(feature.summaryRes)) },
        leadingContent = { Icon(painterResource(feature.iconRes), contentDescription = null) },
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
