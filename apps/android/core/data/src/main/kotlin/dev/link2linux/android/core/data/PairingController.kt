package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.QrPairingOffer
import kotlinx.coroutines.flow.StateFlow

/**
 * What the pairing screens drive. The phone never dials: it listens, and the
 * desktop connects to it. Starting a pairing makes the phone accept that
 * desktop and wait for it; [state] follows the attempt from there.
 *
 * Only one attempt runs at a time. Starting a new one replaces the current
 * one.
 */
interface PairingController {

    /** The current attempt, or [PairingState.Idle]. */
    val state: StateFlow<PairingState>

    /**
     * The user scanned a desktop's QR code. The phone now accepts that
     * desktop's key, for 5 minutes or one pairing, and waits for it to dial.
     */
    fun startQrPairing(offer: QrPairingOffer)

    /**
     * The user turned on "pair without camera". For 2 minutes the phone
     * accepts one pairing connection at a time from any desktop, and gives
     * up after 3 attempts; the user then has to turn it on again.
     */
    fun startSasPairing()

    /** The user's answer to [PairingState.ConfirmCode]: whether both screens show the same code. */
    fun confirmCode(matches: Boolean)

    /** Stops waiting for a desktop, and ends an attempt in progress without storing anything. */
    fun cancel()

    /** Returns to [PairingState.Idle] after [PairingState.Paired] or [PairingState.Failed]. */
    fun dismiss()

    /**
     * Removes the pairing with the desktop whose key is [fingerprint]. If it
     * is connected, it is told first, so it removes its record too.
     */
    suspend fun unpair(fingerprint: KeyFingerprint)
}

/** The phone's listener and its sessions, as discovery needs them. */
interface DesktopConnections {

    /** The TCP port the phone listens on, or null while it does not listen. */
    val listeningPort: StateFlow<Int?>

    /** Fingerprints of the paired desktops that have a session right now. */
    val connected: StateFlow<Set<KeyFingerprint>>
}
