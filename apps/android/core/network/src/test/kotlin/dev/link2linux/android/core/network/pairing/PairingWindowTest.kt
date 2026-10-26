package dev.link2linux.android.core.network.pairing

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.QrPairingOffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

class PairingWindowTest {

    private var clock = 1_000L
    private val window = PairingWindow { clock }
    private val desktop = KeyFingerprint.of(ByteArray(32) { 1 })
    private val other = KeyFingerprint.of(ByteArray(32) { 2 })
    private val offer = QrPairingOffer(desktop, PairingSecret.of(ByteArray(16)), "Desk")
    private val desk = InetAddress.getByName("192.168.1.20")

    @Test
    fun nothingIsAcceptedWhileClosed() {
        assertFalse(window.acceptsKey(desktop))
        assertFalse(window.claimSas(desk))
        assertNull(window.qrOfferFor(desktop))
    }

    @Test
    fun aQrOfferAcceptsOnlyItsDesktopForFiveMinutes() {
        assertEquals(PairingState.AwaitingDesktop.Qr(offer, clock + 300_000), window.openQr(offer))
        assertTrue(window.acceptsKey(desktop))
        assertFalse(window.acceptsKey(other))
        assertEquals(offer, window.qrOfferFor(desktop))
        assertNull(window.qrOfferFor(other))
        assertFalse(window.claimSas(desk))

        clock += 300_000
        assertFalse(window.acceptsKey(desktop))
        assertNull(window.waitingState())
    }

    @Test
    fun sasModeTakesOneConnectionAtATime() {
        window.openSas()
        assertTrue(window.acceptsKey(other))
        assertTrue(window.claimSas(desk))
        assertFalse(window.acceptsKey(other))
        assertFalse(window.claimSas(desk))

        assertEquals(PairingState.AwaitingDesktop.Sas(clock + 120_000, 3, PairingFailure.DECLINED), window.sasFailed(PairingFailure.DECLINED))
        assertTrue(window.claimSas(desk))
    }

    @Test
    fun aSourceThatKeepsLeavingBeforeTheNonceIsRefusedForTheRestOfTheWindow() {
        window.openSas()
        repeat(PairingWindow.MAX_ABANDONED_SAS_CLAIMS) {
            assertTrue(window.claimSas(InetAddress.getByName("fd00::1")))
            window.sasFailed(PairingFailure.CONNECTION_LOST)
        }
        assertFalse(window.claimSas(InetAddress.getByName("fd00::1")))
        // Another address in the same /64 is the same host.
        assertFalse(window.claimSas(InetAddress.getByName("fd00::2")))

        // The user's desktop still gets the slot, and runs that reach the nonce are not held against it.
        repeat(3) {
            assertTrue(window.claimSas(desk))
            if (it < 2) window.sasAttemptUsed()
            window.sasFailed(PairingFailure.DECLINED)
        }
        assertTrue(window.claimSas(desk))
    }

    @Test
    fun aFailureBeforeTheRevealCostsNoAttempt() {
        window.openSas()
        window.claimSas(desk)
        val state = window.sasFailed(PairingFailure.VERIFICATION_FAILED) as PairingState.AwaitingDesktop.Sas
        assertEquals(3, state.attemptsLeft)
    }

    @Test
    fun threeRevealedNoncesEndSasMode() {
        window.openSas()
        repeat(2) {
            assertTrue(window.claimSas(desk))
            window.sasAttemptUsed()
            assertTrue(window.sasFailed(PairingFailure.CONNECTION_LOST) is PairingState.AwaitingDesktop.Sas)
        }
        assertTrue(window.claimSas(desk))
        window.sasAttemptUsed()
        assertEquals(PairingState.Failed(PairingFailure.DECLINED), window.sasFailed(PairingFailure.DECLINED))
        assertFalse(window.acceptsKey(other))
        assertFalse(window.claimSas(desk))
    }

    @Test
    fun sasModeEndsAfterTwoMinutesEvenMidAttempt() {
        window.openSas()
        assertTrue(window.claimSas(desk))
        clock += 120_000
        assertEquals(PairingState.Failed(PairingFailure.TIMED_OUT), window.sasFailed(PairingFailure.TIMED_OUT))
        assertFalse(window.claimSas(desk))
    }

    @Test
    fun aNewWindowReplacesTheOld() {
        window.openQr(offer)
        window.openSas()
        assertNull(window.qrOfferFor(desktop))
        window.close()
        assertNull(window.until)
    }
}
