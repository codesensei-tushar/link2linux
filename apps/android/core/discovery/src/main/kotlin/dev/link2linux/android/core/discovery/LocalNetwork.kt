package dev.link2linux.android.core.discovery

import kotlinx.coroutines.flow.StateFlow
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress

/** An address of the phone on a network, with the length of its subnet prefix. */
data class NetworkAddress(val address: InetAddress, val prefixLength: Int)

/**
 * A Wi-Fi or Ethernet network the phone is on. [handle] is the platform's
 * network object, which the sender binds its sockets to; it is null in tests.
 */
data class LocalNetwork(
    val id: Long,
    val addresses: List<NetworkAddress>,
    val handle: Any? = null,
)

/** The Wi-Fi and Ethernet networks the phone is on, whether or not they reach the internet. */
interface LocalNetworks {
    val networks: StateFlow<List<LocalNetwork>>
}

/** Address arithmetic for beacons. */
object Subnets {

    /** The directed broadcast address of an IPv4 subnet, or null for prefixes that have none. */
    fun broadcastAddress(address: Inet4Address, prefixLength: Int): Inet4Address? {
        if (prefixLength !in 1..30) return null
        val mask = (-1 shl (32 - prefixLength))
        val value = address.address.fold(0) { acc, b -> (acc shl 8) or (b.toInt() and 0xff) }
        val broadcast = value or mask.inv()
        val bytes = ByteArray(4) { i -> (broadcast ushr (24 - 8 * i)).toByte() }
        return InetAddress.getByAddress(bytes) as Inet4Address
    }

    /** The directed broadcast addresses of [network]'s IPv4 subnets. */
    fun broadcastAddresses(network: LocalNetwork): List<Inet4Address> =
        network.addresses.mapNotNull { (a, prefix) -> (a as? Inet4Address)?.let { broadcastAddress(it, prefix) } }

    /** Whether [target] is in a subnet of [network]; link-local IPv6 never counts, as its interface is unknown. */
    fun isOnLink(network: LocalNetwork, target: InetAddress): Boolean {
        if (target is Inet6Address && target.isLinkLocalAddress) return false
        return network.addresses.any { (own, prefix) ->
            own.javaClass == target.javaClass && sharesPrefix(own.address, target.address, prefix)
        }
    }

    private fun sharesPrefix(a: ByteArray, b: ByteArray, prefixLength: Int): Boolean {
        if (a.size != b.size || prefixLength !in 0..a.size * 8) return false
        val whole = prefixLength / 8
        for (i in 0 until whole) if (a[i] != b[i]) return false
        val rest = prefixLength % 8
        if (rest == 0) return true
        val mask = (0xff shl (8 - rest)) and 0xff
        return (a[whole].toInt() and mask) == (b[whole].toInt() and mask)
    }
}
