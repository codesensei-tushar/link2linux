package dev.link2linux.android.feature.files

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** Files sent between the phone and the computer. Placeholder until the feature is built. */
@Composable
fun FilesScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.FILES, onBack = onBack, modifier = modifier)
}
