package dev.link2linux.android.feature.status

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object StatusRoute : NavKey

fun EntryProviderScope<NavKey>.statusEntry(onBack: () -> Unit) {
    entry<StatusRoute> {
        StatusScreen(onBack = onBack)
    }
}
