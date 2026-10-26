package dev.link2linux.android.core.data

import java.net.InetAddress

/**
 * The phone's TCP listener, which every desktop connection arrives on. Only
 * the connection service runs it, as [ConnectionPolicy.shouldListen] says:
 * while the user's switch is on, the phone has a pairing (or a pairing is
 * under way) and is on a local network (Wi-Fi, Ethernet, or one it hosts as
 * a hotspot or by tethering); it is stopped otherwise. It takes connections
 * only from the hosts [ListenAddresses] allows.
 *
 * [DesktopConnections.listeningPort] reports the port it got.
 */
interface DesktopListener {

    /** Starts listening, unless it already does. */
    suspend fun start()

    /** Stops listening and ends every session with a `Goodbye`. */
    suspend fun stop()
}

/**
 * Which connections the listener takes: to an address the phone has on a
 * local network it is on or hosts, from a host on that network or at a
 * private address of a routed local network. Any other, such as one on
 * mobile data or a VPN, or one from the internet to the phone's global IPv6
 * address, is closed before the TLS handshake.
 */
fun interface ListenAddresses {

    /** Whether a connection from [remote] to [local], one of the phone's own addresses, came in over a local network. */
    fun isLocal(local: InetAddress, remote: InetAddress): Boolean
}
