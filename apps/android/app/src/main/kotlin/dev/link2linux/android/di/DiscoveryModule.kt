package dev.link2linux.android.di

import android.content.Context
import android.os.SystemClock
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.discovery.ConnectionSwitch
import dev.link2linux.android.core.discovery.DiscoveryController
import dev.link2linux.android.core.discovery.DiscoveryStatus
import dev.link2linux.android.core.discovery.android.BeaconWakeLock
import dev.link2linux.android.core.discovery.android.ConnectivityLocalNetworks
import dev.link2linux.android.core.discovery.android.NsdAdvertiser
import dev.link2linux.android.core.discovery.android.PairingMulticastLock
import dev.link2linux.android.core.discovery.android.UdpBeaconSender
import dev.link2linux.android.service.PreferencesConnectionSwitch
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

    /** The listener takes connections only over these networks, from hosts on them. */
    @Provides
    fun listenAddresses(networks: ConnectivityLocalNetworks): ListenAddresses = ListenAddresses(networks::isLocalConnection)

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
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Default.limitedParallelism(1) + loggingUncaught("discovery")),
        pairedDevices = pairedDevices,
        pairing = pairing,
        connections = connections,
        localNetworks = networks,
        advertiser = NsdAdvertiser(context),
        sender = UdpBeaconSender(),
        wakeLock = BeaconWakeLock(context),
        multicastLock = PairingMulticastLock(context),
        elapsedRealtimeMillis = SystemClock::elapsedRealtime,
        // Android before 14 needs a new registration whenever a network changes.
        reregisterOnNetworkChange = android.os.Build.VERSION.SDK_INT < 34,
    )

    @Provides
    @Singleton
    fun connectionSwitch(@ApplicationContext context: Context): ConnectionSwitch =
        PreferencesConnectionSwitch(context)

    @Provides
    fun discoveryStatus(controller: DiscoveryController): DiscoveryStatus = controller
}
