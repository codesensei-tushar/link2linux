package dev.link2linux.android.feature.calls

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** Phone calls, placed and answered from the computer. Placeholder until the feature is built. */
@Composable
fun CallsScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.CALLS, onBack = onBack, modifier = modifier)
}
