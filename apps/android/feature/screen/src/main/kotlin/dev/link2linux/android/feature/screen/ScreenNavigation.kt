package dev.link2linux.android.feature.screen

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object ScreenRoute : NavKey

fun EntryProviderScope<NavKey>.screenEntry(onBack: () -> Unit) {
    entry<ScreenRoute> {
        ScreenFeatureScreen(onBack = onBack)
    }
}
