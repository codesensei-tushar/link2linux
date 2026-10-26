package dev.link2linux.android.core.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress

class SubnetsTest {

    private fun ip(text: String) = InetAddress.getByName(text)

    private val network = LocalNetwork(
        1,
        listOf(NetworkAddress(ip("192.168.1.5"), 24), NetworkAddress(ip("fd00::5"), 64), NetworkAddress(ip("fe80::1"), 64)),
    )

    @Test
    fun broadcastAddressOfASubnet() {
        assertEquals(ip("192.168.1.255"), Subnets.broadcastAddress(ip("192.168.1.5") as Inet4Address, 24))
        assertEquals(ip("10.3.255.255"), Subnets.broadcastAddress(ip("10.3.4.5") as Inet4Address, 16))
        assertEquals(ip("172.16.0.7"), Subnets.broadcastAddress(ip("172.16.0.5") as Inet4Address, 29))
        assertNull(Subnets.broadcastAddress(ip("10.0.0.1") as Inet4Address, 32))
        assertNull(Subnets.broadcastAddress(ip("10.0.0.1") as Inet4Address, 31))
        assertNull(Subnets.broadcastAddress(ip("10.0.0.1") as Inet4Address, 4))
        assertEquals(listOf(ip("192.168.1.255")), Subnets.broadcastAddresses(network))
    }

    @Test
    fun onLinkChecksThePrefix() {
        assertTrue(Subnets.isOnLink(network, ip("192.168.1.200")))
        assertFalse(Subnets.isOnLink(network, ip("192.168.2.1")))
        assertTrue(Subnets.isOnLink(network, ip("fd00::20")))
        assertFalse(Subnets.isOnLink(network, ip("fd00:0:0:1::20")))
        assertFalse(Subnets.isOnLink(network, ip("fe80::2"))) // link-local: interface unknown
    }

    @Test
    fun aScannedPublicAddressGetsNoBeacons() {
        val port = 27381
        val public = listOf(InetSocketAddress(ip("203.0.113.9"), port), InetSocketAddress(ip("2001:db8::9"), 9))
        val targets = Subnets.beaconTargets(network, public, port)
        assertTrue(public.none { it in targets })

        // Not even through an interface whose prefix spans half the internet.
        val wide = LocalNetwork(2, listOf(NetworkAddress(ip("10.0.0.5"), 1), NetworkAddress(ip("2001:db8::5"), 16)))
        assertFalse(Subnets.isOnLink(wide, ip("100.64.0.1")))
        assertFalse(Subnets.isOnLink(wide, ip("2001:db8:ffff::1")))
        assertTrue(Subnets.isOnLink(LocalNetwork(3, listOf(NetworkAddress(ip("10.0.0.5"), 8))), ip("10.200.0.1")))
    }

    @Test
    fun joinedNetworkGetsBothBroadcastsAndOnLinkUnicast() {
        val port = 27381
        val onLink = InetSocketAddress(ip("192.168.1.20"), port)
        val elsewhere = InetSocketAddress(ip("10.0.0.9"), port)
        assertEquals(
            setOf(InetSocketAddress(ip("255.255.255.255"), port), InetSocketAddress(ip("192.168.1.255"), port), onLink),
            Subnets.beaconTargets(network, listOf(onLink, elsewhere), port),
        )
    }

    @Test
    fun tetheredNetworkSkipsTheLimitedBroadcast() {
        val port = 27381
        val hotspot = LocalNetwork(-5, listOf(NetworkAddress(ip("192.168.43.1"), 24)), tethered = true)
        val laptop = InetSocketAddress(ip("192.168.43.77"), port)
        assertEquals(
            setOf(InetSocketAddress(ip("192.168.43.255"), port), laptop),
            Subnets.beaconTargets(hotspot, listOf(laptop, InetSocketAddress(ip("192.168.1.20"), port)), port),
        )
    }

    @Test
    fun theNetworksOwnTheirAddressesAndNoOthers() {
        val networks = listOf(network)
        assertTrue(networks.ownsAddress(ip("192.168.1.5")))
        assertTrue(networks.ownsAddress(ip("fd00::5")))
        assertTrue(networks.ownsAddress(ip("fe80::1")))
        // An IPv4 connection on a dual-stack socket can report its address IPv4-mapped.
        val mapped = Inet6Address.getByAddress(null, ByteArray(10) + byteArrayOf(-1, -1, -64, -88, 1, 5), -1)
        assertTrue(mapped is Inet6Address && networks.ownsAddress(mapped))
        // Another address in the same subnet is the desktop's, not the phone's.
        assertFalse(networks.ownsAddress(ip("192.168.1.6")))
        assertFalse(networks.ownsAddress(ip("100.64.0.1")))
        assertFalse(networks.ownsAddress(ip("127.0.0.1")))
        assertFalse(emptyList<LocalNetwork>().ownsAddress(ip("192.168.1.5")))
    }
}
