package dev.link2linux.android.feature.ring

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object RingRoute : NavKey

fun EntryProviderScope<NavKey>.ringEntry(onBack: () -> Unit) {
    entry<RingRoute> {
        RingScreen(onBack = onBack)
    }
}
