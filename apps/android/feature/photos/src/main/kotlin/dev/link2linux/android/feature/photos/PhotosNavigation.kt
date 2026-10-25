package dev.link2linux.android.feature.photos

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

@Serializable
data object PhotosRoute : NavKey

fun EntryProviderScope<NavKey>.photosEntry(onBack: () -> Unit) {
    entry<PhotosRoute> {
        PhotosScreen(onBack = onBack)
    }
}
