package dev.link2linux.android.feature.pairing

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import kotlinx.serialization.Serializable

/** The app's start screen: paired computers, or how to pair one. */
@Serializable
data object PairingRoute : NavKey

fun EntryProviderScope<NavKey>.pairingEntry() {
    entry<PairingRoute> {
        PairingScreen()
    }
}
