package dev.link2linux.android.feature.messages

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import dev.link2linux.android.core.designsystem.component.FeaturePlaceholderScreen
import dev.link2linux.android.core.model.Feature

/** Text messages, read and answered from the computer. Placeholder until the feature is built. */
@Composable
fun MessagesScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FeaturePlaceholderScreen(feature = Feature.MESSAGES, onBack = onBack, modifier = modifier)
}
