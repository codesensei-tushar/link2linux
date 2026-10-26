package dev.link2linux.android.core.data

/**
 * The phone's TCP listener, which every desktop connection arrives on. The
 * connection service runs it: started while the phone has a pairing (or a
 * pairing is under way) and is on a Wi-Fi or Ethernet network, stopped when
 * it leaves. Starting a pairing through [PairingController] starts it too.
 *
 * [DesktopConnections.listeningPort] reports the port it got.
 */
interface DesktopListener {

    /** Starts listening, unless it already does. */
    suspend fun start()

    /** Stops listening and ends every session with a `Goodbye`. */
    suspend fun stop()
}
