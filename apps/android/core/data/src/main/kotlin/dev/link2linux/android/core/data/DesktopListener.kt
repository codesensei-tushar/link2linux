package dev.link2linux.android.core.data

import java.net.InetAddress

/**
 * The phone's TCP listener, which every desktop connection arrives on. The
 * connection service runs it: started while the phone has a pairing (or a
 * pairing is under way) and is on a local network (Wi-Fi, Ethernet, or one
 * it hosts as a hotspot or by tethering), stopped when it leaves. Starting a
 * pairing through [PairingController] starts it too. It takes connections
 * only on the addresses [ListenAddresses] allows.
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
 * Which of the phone's own addresses the listener takes connections on: the
 * ones it has on the local networks it is on or hosts. A connection that
 * arrives on any other address, such as one on mobile data or a VPN, is
 * closed before the TLS handshake.
 */
fun interface ListenAddresses {

    /** Whether a connection to [address], one of the phone's own, came in over a local network. */
    fun isLocal(address: InetAddress): Boolean
}
