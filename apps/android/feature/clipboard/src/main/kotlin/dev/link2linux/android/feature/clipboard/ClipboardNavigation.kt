package dev.link2linux.android.feature.clipboard

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object ClipboardRoute : NavKey

fun EntryProviderScope<NavKey>.clipboardEntry(onBack: () -> Unit) {
    entry<ClipboardRoute> {
        ClipboardScreen(onBack = onBack)
    }
}
