package dev.link2linux.android.feature.messages

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object MessagesRoute : NavKey

fun EntryProviderScope<NavKey>.messagesEntry(onBack: () -> Unit) {
    entry<MessagesRoute> {
        MessagesScreen(onBack = onBack)
    }
}
