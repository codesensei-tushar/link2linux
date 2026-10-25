package dev.link2linux.android.core.data.di

import android.content.Context
import android.os.Build
import android.provider.Settings
import dagger.Binds
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.FeatureSettingsRepository
import dev.link2linux.android.core.data.InMemoryFeatureSettingsRepository
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.store.PersistentPairingStore
import dev.link2linux.android.core.security.KeystoreSecretBox
import java.io.File
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
internal abstract class DataModule {

    @Binds
    abstract fun bindsPairedDeviceRepository(impl: PersistentPairingStore): PairedDeviceRepository

    @Binds
    abstract fun bindsLocalDeviceRepository(impl: PersistentPairingStore): LocalDeviceRepository

    @Binds
    abstract fun bindsFeatureSettingsRepository(impl: InMemoryFeatureSettingsRepository): FeatureSettingsRepository

    companion object {
        @Provides
        @Singleton
        fun providesPairingStore(@ApplicationContext context: Context): PersistentPairingStore =
            PersistentPairingStore(
                // No-backup storage: the pairings are useless without this
                // phone's Keystore keys, and must not be restored elsewhere.
                file = File(context.noBackupFilesDir, "pairing/pairings.bin"),
                box = KeystoreSecretBox(),
                defaultName = { defaultDeviceName(context) },
            )

        /** The name the user gave the phone in its settings, or its model. */
        private fun defaultDeviceName(context: Context): String =
            runCatching { Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) }
                .getOrNull()
                ?.takeIf { it.isNotBlank() }
                ?: Build.MODEL
    }
}
