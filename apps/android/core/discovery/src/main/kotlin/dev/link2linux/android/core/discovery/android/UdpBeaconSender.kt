package dev.link2linux.android.core.discovery.android

import android.net.Network
import dev.link2linux.android.core.discovery.BeaconSender
import dev.link2linux.android.core.discovery.DiscoveryConstants
import dev.link2linux.android.core.discovery.LocalNetwork
import dev.link2linux.android.core.discovery.Subnets
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runInterruptible
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress

/**
 * Sends beacons as UDP datagrams, once per network with a socket bound to that
 * network, so a phone on Wi-Fi and Ethernet at once reaches both. Each
 * datagram goes to 255.255.255.255 and the subnet's broadcast address (some
 * access points drop one of them) and, by unicast, to the desktop's
 * addresses that are on that network.
 */
class UdpBeaconSender : BeaconSender {

    override suspend fun send(networks: List<LocalNetwork>, datagram: ByteArray, unicast: List<InetSocketAddress>) {
        for (network in networks) {
            val targets = LinkedHashSet<InetSocketAddress>()
            targets += InetSocketAddress(LIMITED_BROADCAST, DiscoveryConstants.BEACON_PORT)
            Subnets.broadcastAddresses(network).forEach {
                targets += InetSocketAddress(it, DiscoveryConstants.BEACON_PORT)
            }
            unicast.filterTo(targets) { Subnets.isOnLink(network, it.address) }
            sendOn(network, datagram, targets)
        }
    }

    private suspend fun sendOn(network: LocalNetwork, datagram: ByteArray, targets: Collection<InetSocketAddress>) {
        runInterruptible(Dispatchers.IO) {
            try {
                DatagramSocket().use { socket ->
                    (network.handle as? Network)?.bindSocket(socket)
                    socket.broadcast = true
                    for (target in targets) {
                        try {
                            socket.send(DatagramPacket(datagram, datagram.size, target))
                        } catch (_: IOException) {
                            // Best effort: one unreachable target does not stop the others.
                        }
                    }
                }
            } catch (_: IOException) {
                // The network went away while we were binding.
            }
        }
    }

    private companion object {
        val LIMITED_BROADCAST: InetAddress = Inet4Address.getByAddress(byteArrayOf(-1, -1, -1, -1))
    }
}
