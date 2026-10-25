package dev.link2linux.android.feature.calls

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object CallsRoute : NavKey

fun EntryProviderScope<NavKey>.callsEntry(onBack: () -> Unit) {
    entry<CallsRoute> {
        CallsScreen(onBack = onBack)
    }
}
