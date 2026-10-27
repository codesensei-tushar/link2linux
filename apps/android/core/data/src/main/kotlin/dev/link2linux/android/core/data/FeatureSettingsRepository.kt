package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.Feature
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Which features the user has on. Each starts as [Feature.isOnByDefault]
 * says; an opt-in feature's screen asks for its Android permissions before
 * turning it on.
 */
interface FeatureSettingsRepository {

    val enabledFeatures: Flow<Set<Feature>>

    /** @throws IllegalArgumentException for [Feature.PAIRING], which is always on. */
    suspend fun setEnabled(feature: Feature, enabled: Boolean)
}

/** Keeps the settings in memory until persistent storage lands. */
@Singleton
internal class InMemoryFeatureSettingsRepository @Inject constructor() : FeatureSettingsRepository {

    private val enabled = MutableStateFlow(Feature.entries.filter(Feature::isOnByDefault).toSet())

    override val enabledFeatures: StateFlow<Set<Feature>> = enabled.asStateFlow()

    override suspend fun setEnabled(feature: Feature, enabled: Boolean) {
        require(feature.isSwitchable) { "$feature cannot be turned off" }
        this.enabled.update { if (enabled) it + feature else it - feature }
    }
}
