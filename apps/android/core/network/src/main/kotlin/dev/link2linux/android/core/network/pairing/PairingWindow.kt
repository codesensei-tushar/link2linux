package dev.link2linux.android.core.network.pairing

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.QrPairingOffer

/**
 * Whether the phone is waiting for a desktop to pair, and with whom: the
 * rules of docs/protocol.md that decide which pairing connections it takes.
 * Thread-safe; [acceptsKey] is called on the TLS handshake thread.
 */
internal class PairingWindow(private val now: () -> Long) {

    private sealed interface Open {
        val until: Long
    }

    private data class Qr(val offer: QrPairingOffer, override val until: Long) : Open

    private data class Sas(
        override val until: Long,
        val attemptsLeft: Int,
        val busy: Boolean,
        val lastFailure: PairingFailure?,
    ) : Open

    @Volatile
    private var open: Open? = null

    /** Waits for the desktop of [offer]; forgets it after [QR_WINDOW_MILLIS] or a pairing. */
    @Synchronized
    fun openQr(offer: QrPairingOffer): PairingState.AwaitingDesktop.Qr {
        val window = Qr(offer, now() + QR_WINDOW_MILLIS)
        open = window
        return PairingState.AwaitingDesktop.Qr(offer, window.until)
    }

    /** Turns on SAS mode: [SAS_WINDOW_MILLIS], [SAS_ATTEMPTS] attempts, one connection at a time. */
    @Synchronized
    fun openSas(): PairingState.AwaitingDesktop.Sas {
        val window = Sas(now() + SAS_WINDOW_MILLIS, SAS_ATTEMPTS, busy = false, lastFailure = null)
        open = window
        return PairingState.AwaitingDesktop.Sas(window.until, window.attemptsLeft)
    }

    @Synchronized
    fun close() {
        open = null
    }

    /** When the open window ends, or null. */
    val until: Long? get() = open?.until

    /** Whether a desktop with [key] may complete TLS to pair. */
    fun acceptsKey(key: KeyFingerprint): Boolean = when (val window = open) {
        is Qr -> window.offer.desktopFingerprint == key && now() < window.until
        is Sas -> now() < window.until && !window.busy && window.attemptsLeft > 0
        null -> false
    }

    /** The offer a QR pairing connection from [key] pairs against, or null if none is waiting for it. */
    @Synchronized
    fun qrOfferFor(key: KeyFingerprint): QrPairingOffer? =
        (open as? Qr)?.takeIf { it.offer.desktopFingerprint == key && now() < it.until }?.offer

    /** Takes the single SAS slot. False if SAS mode is off, used up, or busy with another connection. */
    @Synchronized
    fun claimSas(): Boolean {
        val window = open as? Sas ?: return false
        if (window.busy || window.attemptsLeft <= 0 || now() >= window.until) return false
        open = window.copy(busy = true)
        return true
    }

    /** The phone revealed its nonce: the running SAS connection uses up an attempt. */
    @Synchronized
    fun sasAttemptUsed() {
        val window = open as? Sas ?: return
        open = window.copy(attemptsLeft = window.attemptsLeft - 1)
    }

    /**
     * The SAS connection ended without a pairing. Returns the state to show:
     * waiting again while time and attempts are left, otherwise failed with
     * SAS mode off.
     */
    @Synchronized
    fun sasFailed(failure: PairingFailure): PairingState {
        val window = open as? Sas ?: return PairingState.Failed(failure)
        if (window.attemptsLeft <= 0 || now() >= window.until) {
            open = null
            return PairingState.Failed(failure)
        }
        open = window.copy(busy = false, lastFailure = failure)
        return PairingState.AwaitingDesktop.Sas(window.until, window.attemptsLeft, failure)
    }

    /** The state to show while waiting, or null when no window is open or it has run out. */
    @Synchronized
    fun waitingState(): PairingState.AwaitingDesktop? = when (val window = open) {
        is Qr -> PairingState.AwaitingDesktop.Qr(window.offer, window.until).takeIf { now() < window.until }
        is Sas -> PairingState.AwaitingDesktop.Sas(window.until, window.attemptsLeft, window.lastFailure)
            .takeIf { now() < window.until && window.attemptsLeft > 0 }
        null -> null
    }

    /** Whether a SAS connection is running right now. */
    val sasBusy: Boolean get() = (open as? Sas)?.busy == true

    companion object {
        /** The phone forgets a scanned offer 5 minutes after the scan. */
        const val QR_WINDOW_MILLIS: Long = 5 * 60_000

        /** SAS mode lasts 2 minutes. */
        const val SAS_WINDOW_MILLIS: Long = 2 * 60_000

        /** Attempts per activation of SAS mode. */
        const val SAS_ATTEMPTS: Int = 3
    }
}
