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
 * Which opt-in features the user has turned on. Every feature starts off;
 * the feature screen asks for its Android permissions before turning on.
 */
interface FeatureSettingsRepository {

    val enabledFeatures: Flow<Set<Feature>>

    /** @throws IllegalArgumentException for [Feature.PAIRING], which is not opt-in. */
    suspend fun setEnabled(feature: Feature, enabled: Boolean)
}

/** Keeps the settings in memory until persistent storage lands. */
@Singleton
internal class InMemoryFeatureSettingsRepository @Inject constructor() : FeatureSettingsRepository {

    private val enabled = MutableStateFlow<Set<Feature>>(emptySet())

    override val enabledFeatures: StateFlow<Set<Feature>> = enabled.asStateFlow()

    override suspend fun setEnabled(feature: Feature, enabled: Boolean) {
        require(feature.isOptIn) { "$feature cannot be turned off" }
        this.enabled.update { if (enabled) it + feature else it - feature }
    }
}
