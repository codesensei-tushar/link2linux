package dev.link2linux.android.feature.photos

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** Recent photos and screenshots, browsed from the computer. Placeholder until the feature is built. */
@Composable
fun PhotosScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.PHOTOS, onBack = onBack, modifier = modifier)
}
