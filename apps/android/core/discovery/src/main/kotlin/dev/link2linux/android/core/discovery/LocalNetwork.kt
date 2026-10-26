package dev.link2linux.android.core.discovery

import kotlinx.coroutines.flow.StateFlow
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress

/** An address of the phone on a network, with the length of its subnet prefix. */
data class NetworkAddress(val address: InetAddress, val prefixLength: Int)

/**
 * A network the phone is on or serves. [handle] is the platform's network
 * object, which the sender binds its sockets to; it is null in tests and for
 * [tethered] networks, which the platform does not expose as objects.
 *
 * A tethered network is one the phone hosts for a computer: its Wi-Fi
 * hotspot, or USB or Bluetooth tethering. Its [id] is negative, so it never
 * equals the id of a platform network. Its [addresses] are only the private
 * IPv4 ones that make it count; [ownAddresses] has every address of its
 * interface, as a desktop may dial any of them.
 */
data class LocalNetwork(
    val id: Long,
    val addresses: List<NetworkAddress>,
    val handle: Any? = null,
    val tethered: Boolean = false,
    /** Every address the phone has on this network: the ones the listener takes connections on. */
    val ownAddresses: List<InetAddress> = addresses.map { it.address },
)

/** Whether [address] is one the phone has on one of these networks. An IPv4-mapped IPv6 address counts as its IPv4 address. */
fun List<LocalNetwork>.ownsAddress(address: InetAddress): Boolean {
    val wanted = Subnets.unmapped(address.address)
    return any { network -> network.ownAddresses.any { Subnets.unmapped(it.address).contentEquals(wanted) } }
}

/**
 * The networks the phone is on, whether or not they reach the internet: Wi-Fi
 * and Ethernet it has joined, and the hotspot or tethering networks it hosts.
 */
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

    /**
     * Where a beacon for [network] goes: the subnet broadcast addresses, and
     * the [unicast] ones that are on the network. A network the phone joined
     * also gets 255.255.255.255, through the socket bound to it (some access
     * points drop one of the two). A tethered network is reached through an
     * unbound socket, which would send 255.255.255.255 out of the default
     * route instead, so it gets the directed broadcast only.
     */
    fun beaconTargets(network: LocalNetwork, unicast: List<InetSocketAddress>, port: Int): Set<InetSocketAddress> {
        val targets = LinkedHashSet<InetSocketAddress>()
        if (!network.tethered) targets += InetSocketAddress(LIMITED_BROADCAST, port)
        broadcastAddresses(network).forEach { targets += InetSocketAddress(it, port) }
        unicast.filterTo(targets) { isOnLink(network, it.address) }
        return targets
    }

    private val LIMITED_BROADCAST: InetAddress = InetAddress.getByAddress(byteArrayOf(-1, -1, -1, -1))

    /** Whether [target] is in a subnet of [network]; link-local IPv6 never counts, as its interface is unknown. */
    fun isOnLink(network: LocalNetwork, target: InetAddress): Boolean {
        if (target is Inet6Address && target.isLinkLocalAddress) return false
        return network.addresses.any { (own, prefix) ->
            own.javaClass == target.javaClass && sharesPrefix(own.address, target.address, prefix)
        }
    }

    /** The 4 bytes of an IPv4-mapped IPv6 address (::ffff:a.b.c.d); any other address as it is. */
    fun unmapped(address: ByteArray): ByteArray {
        val mapped = address.size == 16 && (0 until 10).all { address[it] == 0.toByte() } &&
            address[10] == (-1).toByte() && address[11] == (-1).toByte()
        return if (mapped) address.copyOfRange(12, 16) else address
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
