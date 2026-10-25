package dev.link2linux.android.feature.screen

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** The phone's screen, shown and used on the computer. Placeholder until the feature is built. */
@Composable
fun ScreenFeatureScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.SCREEN, onBack = onBack, modifier = modifier)
}
