package dev.link2linux.android.core.network.di

import dagger.Binds
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.DesktopListener
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.network.pairing.PairingService
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.core.security.AndroidKeyStoreIdentity
import dev.link2linux.android.core.security.IdentityKeySource
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
internal abstract class NetworkModule {

    @Binds
    abstract fun bindsPairingController(impl: PairingService): PairingController

    @Binds
    abstract fun bindsDesktopConnections(impl: PairingService): DesktopConnections

    @Binds
    abstract fun bindsDesktopListener(impl: PairingService): DesktopListener

    companion object {
        @Provides
        @Singleton
        fun providesIdentityKeySource(): IdentityKeySource = AndroidKeyStoreIdentity()

        @Provides
        @Singleton
        fun providesPairingService(
            identity: IdentityKeySource,
            devices: PairedDeviceRepository,
            localDevice: LocalDeviceRepository,
            hooks: SessionHooks,
            listenAddresses: ListenAddresses,
        ): PairingService = PairingService(
            identity = identity,
            devices = devices,
            localDevice = localDevice,
            listenAddresses = listenAddresses,
            // Connections outlive any screen; the service lives as long as the app.
            scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + loggingUncaught("the pairing service")),
            hooks = hooks,
        )
    }
}
