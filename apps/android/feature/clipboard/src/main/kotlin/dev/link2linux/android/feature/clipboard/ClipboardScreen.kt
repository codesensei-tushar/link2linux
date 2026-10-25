package dev.link2linux.android.feature.clipboard

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** One clipboard shared by the phone and the computer. Placeholder until the feature is built. */
@Composable
fun ClipboardScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.CLIPBOARD, onBack = onBack, modifier = modifier)
}
