package dev.link2linux.android.feature.notifications

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object NotificationsRoute : NavKey

fun EntryProviderScope<NavKey>.notificationsEntry(onBack: () -> Unit) {
    entry<NotificationsRoute> {
        NotificationsScreen(onBack = onBack)
    }
}
