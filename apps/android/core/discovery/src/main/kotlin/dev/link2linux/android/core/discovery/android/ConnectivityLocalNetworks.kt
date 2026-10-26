package dev.link2linux.android.core.discovery.android

import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import androidx.core.content.ContextCompat
import dev.link2linux.android.core.discovery.InterfaceSnapshot
import dev.link2linux.android.core.discovery.LocalNetwork
import dev.link2linux.android.core.discovery.LocalNetworks
import dev.link2linux.android.core.discovery.NetworkAddress
import dev.link2linux.android.core.discovery.TetheredNetworks
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.net.NetworkInterface
import java.net.SocketException

/**
 * Follows the networks the phone is on: the Wi-Fi and Ethernet networks it has
 * joined, and the ones it hosts (its Wi-Fi hotspot, USB tethering and
 * Bluetooth tethering). The request does not ask for internet access: a router
 * without an uplink is still a network the desktop can be on. Call [start]
 * once the connection service runs and [stop] when it ends.
 *
 * Android does not report tethering as a network to apps, so the hosted ones
 * are found by looking at the interfaces (see [TetheredNetworks]). That scan
 * runs again on every connectivity callback, on the system's tethering and
 * hotspot state broadcasts, and every [POLL_MILLIS] as a backstop.
 */
class ConnectivityLocalNetworks(context: Context) : LocalNetworks {

    private val appContext = context.applicationContext
    private val connectivity = appContext.getSystemService(ConnectivityManager::class.java)
    private val state = MutableStateFlow<List<LocalNetwork>>(emptyList())
    private var callback: ConnectivityManager.NetworkCallback? = null
    private var receiver: BroadcastReceiver? = null
    private var scope: CoroutineScope? = null

    override val networks: StateFlow<List<LocalNetwork>> = state.asStateFlow()

    private val addresses = HashMap<Network, List<NetworkAddress>>()

    // The app's manifest declares ACCESS_NETWORK_STATE; this library has none of its own.
    @SuppressLint("MissingPermission")
    @Synchronized
    fun start() {
        if (callback != null) return
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) {
                update(network, linkProperties)
            }

            override fun onLost(network: Network) {
                synchronized(this@ConnectivityLocalNetworks) {
                    addresses.remove(network)
                    publish()
                }
            }
        }
        callback = cb
        connectivity.registerNetworkCallback(request, cb)

        val poller = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        scope = poller

        // System broadcasts: not-exported receivers get them. Neither needs a permission.
        val tetheringChanged = object : BroadcastReceiver() {
            // Off the main thread: a rescan asks the system about every network.
            override fun onReceive(context: Context, intent: Intent) {
                poller.launch { rescan() }
            }
        }
        val filter = IntentFilter().apply {
            addAction(ACTION_TETHER_STATE_CHANGED)
            addAction(ACTION_WIFI_AP_STATE_CHANGED)
        }
        ContextCompat.registerReceiver(appContext, tetheringChanged, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
        receiver = tetheringChanged

        poller.launch {
            while (isActive) {
                rescan()
                delay(POLL_MILLIS)
            }
        }
    }

    @Synchronized
    fun stop() {
        callback?.let { connectivity.unregisterNetworkCallback(it) }
        callback = null
        receiver?.let { appContext.unregisterReceiver(it) }
        receiver = null
        scope?.cancel()
        scope = null
        addresses.clear()
        state.value = emptyList()
    }

    @Synchronized
    private fun update(network: Network, properties: LinkProperties) {
        addresses[network] = properties.linkAddresses.map { NetworkAddress(it.address, it.prefixLength) }
        publish()
    }

    /** Looks at the interfaces again, for tethering that came or went. */
    @Synchronized
    private fun rescan() {
        if (callback != null) publish()
    }

    private fun publish() {
        val platform = addresses.map { (network, list) -> LocalNetwork(network.networkHandle, list, network) }
        val tethered = TetheredNetworks.classify(interfaces(), claimedInterfaces())
        val merged = TetheredNetworks.merge(platform, tethered)
        if (merged != state.value) state.value = merged
    }

    /** The names of the interfaces of every network Android knows. Stacked (464XLAT) links are hidden API; their names are excluded by prefix instead. */
    @Suppress("DEPRECATION") // allNetworks: the callback-based replacement would not list networks we did not ask for.
    @SuppressLint("MissingPermission")
    private fun claimedInterfaces(): Set<String> {
        val names = HashSet<String>()
        for (network in connectivity.allNetworks) {
            val properties = connectivity.getLinkProperties(network) ?: continue
            properties.interfaceName?.let(names::add)
        }
        return names
    }

    private fun interfaces(): List<InterfaceSnapshot> = try {
        NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().mapNotNull { nif ->
            try {
                InterfaceSnapshot(
                    name = nif.name,
                    isUp = nif.isUp,
                    isLoopback = nif.isLoopback,
                    addresses = nif.interfaceAddresses.map { NetworkAddress(it.address, it.networkPrefixLength.toInt()) },
                )
            } catch (_: SocketException) {
                null // the interface went away while we looked
            }
        }
    } catch (_: SocketException) {
        emptyList()
    }

    private companion object {
        const val POLL_MILLIS = 10_000L
        const val ACTION_TETHER_STATE_CHANGED = "android.net.conn.TETHER_STATE_CHANGED"
        const val ACTION_WIFI_AP_STATE_CHANGED = "android.net.wifi.WIFI_AP_STATE_CHANGED"
    }
}
