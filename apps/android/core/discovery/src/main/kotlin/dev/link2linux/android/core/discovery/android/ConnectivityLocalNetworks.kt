package dev.link2linux.android.core.discovery.android

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import dev.link2linux.android.core.discovery.LocalNetwork
import dev.link2linux.android.core.discovery.LocalNetworks
import dev.link2linux.android.core.discovery.NetworkAddress
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Follows the Wi-Fi and Ethernet networks the phone is on. The request does
 * not ask for internet access: a router without an uplink is still a network
 * the desktop can be on. Call [start] once the connection service runs and
 * [stop] when it ends.
 */
class ConnectivityLocalNetworks(context: Context) : LocalNetworks {

    private val connectivity = context.applicationContext.getSystemService(ConnectivityManager::class.java)
    private val state = MutableStateFlow<List<LocalNetwork>>(emptyList())
    private var callback: ConnectivityManager.NetworkCallback? = null

    override val networks: StateFlow<List<LocalNetwork>> = state.asStateFlow()

    private val addresses = HashMap<Network, List<NetworkAddress>>()

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
    }

    @Synchronized
    fun stop() {
        callback?.let { connectivity.unregisterNetworkCallback(it) }
        callback = null
        addresses.clear()
        state.value = emptyList()
    }

    @Synchronized
    private fun update(network: Network, properties: LinkProperties) {
        addresses[network] = properties.linkAddresses.map { NetworkAddress(it.address, it.prefixLength) }
        publish()
    }

    private fun publish() {
        state.update {
            addresses.map { (network, list) -> LocalNetwork(network.networkHandle, list, network) }
                .sortedBy { it.id }
        }
    }
}
