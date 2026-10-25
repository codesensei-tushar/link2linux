package dev.link2linux.android.feature.pairing

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import dev.link2linux.android.core.model.Feature
import kotlinx.serialization.Serializable

/** The app's start screen: paired computers, or how to pair one, and the features. */
@Serializable
data object PairingRoute : NavKey

/**
 * @param features the features to list, in order; the app passes the ones it has screens for.
 * @param onOpenFeature opens a feature's screen.
 */
fun EntryProviderScope<NavKey>.pairingEntry(
    features: List<Feature>,
    onOpenFeature: (Feature) -> Unit,
) {
    entry<PairingRoute> {
        PairingScreen(features = features, onOpenFeature = onOpenFeature)
    }
}
