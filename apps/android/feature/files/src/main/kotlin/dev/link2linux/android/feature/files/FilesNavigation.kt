package dev.link2linux.android.feature.files

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object FilesRoute : NavKey

fun EntryProviderScope<NavKey>.filesEntry(onBack: () -> Unit) {
    entry<FilesRoute> {
        FilesScreen(onBack = onBack)
    }
}
