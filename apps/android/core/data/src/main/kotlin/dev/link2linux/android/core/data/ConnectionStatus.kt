package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingState

/** The paired desktops that have a session now, given [DesktopConnections.connected]. */
fun List<PairedDevice>.connectedIn(connected: Set<KeyFingerprint>): List<PairedDevice> =
    filter { desktop -> connected.any(desktop::accepts) }

/** When the connection service keeps the phone's listener running. */
object ListenerPolicy {

    /**
     * The listener runs while the user's connection switch is on, the phone
     * is on a local network, and either a desktop is paired, so it can
     * reconnect, or a pairing is under way, so the desktop can reach the
     * phone. Off a local network (on mobile data only) it is closed.
     */
    fun shouldListen(paired: List<PairedDevice>, pairing: PairingState, switchOn: Boolean, onLocalNetwork: Boolean): Boolean =
        switchOn && onLocalNetwork && (paired.isNotEmpty() || pairing.isUnderWay)

    private val PairingState.isUnderWay: Boolean
        get() = when (this) {
            PairingState.Idle, is PairingState.Paired, is PairingState.Failed -> false
            else -> true
        }
}
