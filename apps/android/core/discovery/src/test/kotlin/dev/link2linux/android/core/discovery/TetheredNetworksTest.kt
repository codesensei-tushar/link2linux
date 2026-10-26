package dev.link2linux.android.core.discovery

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

class TetheredNetworksTest {

    private fun ip(text: String) = InetAddress.getByName(text)

    private fun nic(
        name: String,
        address: String = "192.168.43.1",
        prefix: Int = 24,
        up: Boolean = true,
        loopback: Boolean = false,
    ) = InterfaceSnapshot(name, up, loopback, listOf(NetworkAddress(ip(address), prefix)))

    private fun names(interfaces: List<InterfaceSnapshot>, claimed: Set<String> = emptySet()) =
        TetheredNetworks.classify(interfaces, claimed).map { it.addresses.single().address.hostAddress }

    @Test
    fun hotspotUsbAndBluetoothInterfacesCountWhateverTheirName() {
        val found = TetheredNetworks.classify(
            listOf(
                nic("wlan1", "192.168.43.1"),
                nic("swlan0", "192.168.44.1"),
                nic("ap0", "192.168.45.1"),
                nic("softap0", "192.168.46.1"),
                nic("rndis0", "192.168.42.129"),
                nic("ncm0", "10.1.2.1"),
                nic("bt-pan", "192.168.44.2"),
            ),
            claimed = emptySet(),
        )
        assertEquals(7, found.size)
        assertTrue(found.all { it.tethered && it.handle == null })
    }

    @Test
    fun interfacesOfPlatformNetworksAreLeftToThem() {
        assertEquals(listOf("192.168.43.1"), names(listOf(nic("wlan1"), nic("wlan0", "192.168.1.5")), claimed = setOf("wlan0")))
    }

    @Test
    fun mobileDataVpnAndTranslationInterfacesAreNeverTethering() {
        val never = listOf("rmnet_data1", "ccmni0", "seth_lte0", "sipa_eth0", "tun0", "ipsec0", "clat4", "v4-rmnet_data1", "dummy0", "lo", "p2p-wlan0-0")
        assertEquals(emptyList<String>(), names(never.map { nic(it, "10.0.0.2") }))
    }

    @Test
    fun onlyUpNonLoopbackPrivateIpv4InterfacesCount() {
        assertEquals(emptyList<String>(), names(listOf(nic("wlan1", up = false))))
        assertEquals(emptyList<String>(), names(listOf(nic("wlan1", loopback = true))))
        assertEquals(emptyList<String>(), names(listOf(nic("wlan1", "8.8.8.8"))))
        assertEquals(emptyList<String>(), names(listOf(nic("wlan1", "fe80::1"))))
        assertEquals(emptyList<String>(), names(listOf(nic("wlan1", "fd00::1", 64))))
        assertEquals(emptyList<String>(), names(listOf(nic("wlan1", "192.168.43.1", 32))))
    }

    @Test
    fun theSubnetPrefixComesFromTheInterface() {
        val network = TetheredNetworks.classify(listOf(nic("wlan1", "172.20.10.1", 28)), emptySet()).single()
        assertEquals(28, network.addresses.single().prefixLength)
        assertEquals(listOf(ip("172.20.10.15")), Subnets.broadcastAddresses(network))
    }

    @Test
    fun idsAreStableNegativeAndPerInterface() {
        val a = TetheredNetworks.idOf("wlan1")
        assertEquals(a, TetheredNetworks.idOf("wlan1"))
        assertNotEquals(a, TetheredNetworks.idOf("rndis0"))
        assertTrue(a < 0 && TetheredNetworks.idOf("rndis0") < 0 && TetheredNetworks.idOf("") < 0)
    }

    @Test
    fun mergeKeepsBothSourcesOrderedAndWithoutRepeats() {
        val wifi = LocalNetwork(7, listOf(NetworkAddress(ip("192.168.1.5"), 24)), handle = "wifi")
        val hotspot = TetheredNetworks.classify(listOf(nic("wlan1")), emptySet())
        val merged = TetheredNetworks.merge(listOf(wifi), hotspot + hotspot)
        assertEquals(listOf(hotspot.single().id, 7L), merged.map { it.id })
        assertEquals(listOf(wifi), TetheredNetworks.merge(listOf(wifi), emptyList()))
        assertEquals(hotspot, TetheredNetworks.merge(emptyList(), hotspot))
    }
}
