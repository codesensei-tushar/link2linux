package dev.link2linux.android.core.discovery

import java.net.Inet4Address

/** What the platform reports about one network interface, as [TetheredNetworks] needs it. */
data class InterfaceSnapshot(
    val name: String,
    val isUp: Boolean,
    val isLoopback: Boolean,
    val addresses: List<NetworkAddress>,
)

/**
 * Finds the networks the phone hosts for a computer: its Wi-Fi hotspot and USB
 * or Bluetooth tethering. Android does not hand these to apps as networks, so
 * they are found among the interfaces instead.
 *
 * Interface names vary by manufacturer (wlan1, ap0, swlan0, softap0 for the
 * hotspot; rndis0, ncm0, usb0 for USB; bt-pan for Bluetooth), so no name says
 * "this is a hotspot". An interface counts when it is up, has a private IPv4
 * address, is not one the platform reports as a network (those are tracked
 * with their handles), and is not a known mobile-data, VPN or translation
 * interface.
 */
object TetheredNetworks {

    /** Interface name prefixes that are never tethering: loopback, mobile data, VPNs, 464XLAT and Wi-Fi Direct. */
    private val NEVER = listOf(
        "lo", "rmnet", "r_rmnet", "ccmni", "ccinet", "seth", "sipa", "epdg",
        "tun", "tap", "ipsec", "clat", "v4-", "dummy", "p2p",
    )

    /**
     * The tethered networks among [interfaces], one per interface, ordered by id.
     * [claimed] holds the names of interfaces that belong to a platform
     * network.
     */
    fun classify(interfaces: List<InterfaceSnapshot>, claimed: Set<String>): List<LocalNetwork> =
        interfaces
            .filter { it.isUp && !it.isLoopback && it.name !in claimed && !isExcludedName(it.name) }
            .mapNotNull { snapshot ->
                val addresses = snapshot.addresses.filter { (a, prefix) ->
                    a is Inet4Address && a.isSiteLocalAddress && prefix in 1..30
                }
                if (addresses.isEmpty()) null else LocalNetwork(idOf(snapshot.name), addresses, tethered = true)
            }
            .sortedBy { it.id }

    /** Whether [name] is an interface that is never tethering. */
    fun isExcludedName(name: String): Boolean = NEVER.any { name.lowercase().startsWith(it) }

    /** A stable id for the interface called [name]. It is always negative, as platform handles are not. */
    fun idOf(name: String): Long = Long.MIN_VALUE or (name.hashCode().toLong() and 0xffffffffL)

    /** [platform] networks followed by [tethered] ones, without repeating an id, ordered by id. */
    fun merge(platform: List<LocalNetwork>, tethered: List<LocalNetwork>): List<LocalNetwork> =
        (platform + tethered).distinctBy { it.id }.sortedBy { it.id }
}
