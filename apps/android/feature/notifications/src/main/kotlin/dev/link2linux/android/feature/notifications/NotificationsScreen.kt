package dev.link2linux.android.feature.notifications

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** Phone notifications, mirrored on the computer. Placeholder until the feature is built. */
@Composable
fun NotificationsScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.NOTIFICATIONS, onBack = onBack, modifier = modifier)
}
