package dev.link2linux.android.core.data.di

import dagger.Binds
import dagger.Module
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.FeatureSettingsRepository
import dev.link2linux.android.core.data.InMemoryFeatureSettingsRepository
import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.PairedDeviceRepository

@Module
@InstallIn(SingletonComponent::class)
internal abstract class DataModule {

    @Binds
    abstract fun bindsPairedDeviceRepository(impl: InMemoryPairedDeviceRepository): PairedDeviceRepository

    @Binds
    abstract fun bindsFeatureSettingsRepository(impl: InMemoryFeatureSettingsRepository): FeatureSettingsRepository
}
