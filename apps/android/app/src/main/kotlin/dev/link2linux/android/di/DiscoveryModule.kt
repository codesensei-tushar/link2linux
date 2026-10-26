package dev.link2linux.android.di

import android.content.Context
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.discovery.DiscoveryController
import dev.link2linux.android.core.discovery.DiscoveryStatus
import dev.link2linux.android.core.discovery.android.BeaconWakeLock
import dev.link2linux.android.core.discovery.android.ConnectivityLocalNetworks
import dev.link2linux.android.core.discovery.android.NsdAdvertiser
import dev.link2linux.android.core.discovery.android.PairingMulticastLock
import dev.link2linux.android.core.discovery.android.UdpBeaconSender
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import javax.inject.Singleton

/** Wires discovery: the mDNS registration, the beacon sender, the network callbacks and the locks. */
@Module
@InstallIn(SingletonComponent::class)
internal object DiscoveryModule {

    @Provides
    @Singleton
    fun localNetworks(@ApplicationContext context: Context): ConnectivityLocalNetworks =
        ConnectivityLocalNetworks(context)

    @Provides
    @Singleton
    fun discoveryController(
        @ApplicationContext context: Context,
        pairedDevices: PairedDeviceRepository,
        pairing: PairingController,
        connections: DesktopConnections,
        networks: ConnectivityLocalNetworks,
    ): DiscoveryController = DiscoveryController(
        // One coroutine at a time: the controller keeps plain state.
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Default.limitedParallelism(1)),
        pairedDevices = pairedDevices,
        pairing = pairing,
        connections = connections,
        localNetworks = networks,
        advertiser = NsdAdvertiser(context),
        sender = UdpBeaconSender(),
        wakeLock = BeaconWakeLock(context),
        multicastLock = PairingMulticastLock(context),
        // Android before 14 needs a new registration whenever a network changes.
        reregisterOnNetworkChange = android.os.Build.VERSION.SDK_INT < 34,
    )

    @Provides
    fun discoveryStatus(controller: DiscoveryController): DiscoveryStatus = controller
}
