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

class PairingWindowTest {

    private var clock = 1_000L
    private val window = PairingWindow { clock }
    private val desktop = KeyFingerprint.of(ByteArray(32) { 1 })
    private val other = KeyFingerprint.of(ByteArray(32) { 2 })
    private val offer = QrPairingOffer(desktop, PairingSecret.of(ByteArray(16)), "Desk")

    @Test
    fun nothingIsAcceptedWhileClosed() {
        assertFalse(window.acceptsKey(desktop))
        assertFalse(window.claimSas())
        assertNull(window.qrOfferFor(desktop))
    }

    @Test
    fun aQrOfferAcceptsOnlyItsDesktopForFiveMinutes() {
        assertEquals(PairingState.AwaitingDesktop.Qr(offer, clock + 300_000), window.openQr(offer))
        assertTrue(window.acceptsKey(desktop))
        assertFalse(window.acceptsKey(other))
        assertEquals(offer, window.qrOfferFor(desktop))
        assertNull(window.qrOfferFor(other))
        assertFalse(window.claimSas())

        clock += 300_000
        assertFalse(window.acceptsKey(desktop))
        assertNull(window.waitingState())
    }

    @Test
    fun sasModeTakesOneConnectionAtATime() {
        window.openSas()
        assertTrue(window.acceptsKey(other))
        assertTrue(window.claimSas())
        assertFalse(window.acceptsKey(other))
        assertFalse(window.claimSas())

        assertEquals(PairingState.AwaitingDesktop.Sas(clock + 120_000, 3, PairingFailure.DECLINED), window.sasFailed(PairingFailure.DECLINED))
        assertTrue(window.claimSas())
    }

    @Test
    fun aFailureBeforeTheRevealCostsNoAttempt() {
        window.openSas()
        window.claimSas()
        val state = window.sasFailed(PairingFailure.VERIFICATION_FAILED) as PairingState.AwaitingDesktop.Sas
        assertEquals(3, state.attemptsLeft)
    }

    @Test
    fun threeRevealedNoncesEndSasMode() {
        window.openSas()
        repeat(2) {
            assertTrue(window.claimSas())
            window.sasAttemptUsed()
            assertTrue(window.sasFailed(PairingFailure.CONNECTION_LOST) is PairingState.AwaitingDesktop.Sas)
        }
        assertTrue(window.claimSas())
        window.sasAttemptUsed()
        assertEquals(PairingState.Failed(PairingFailure.DECLINED), window.sasFailed(PairingFailure.DECLINED))
        assertFalse(window.acceptsKey(other))
        assertFalse(window.claimSas())
    }

    @Test
    fun sasModeEndsAfterTwoMinutesEvenMidAttempt() {
        window.openSas()
        assertTrue(window.claimSas())
        clock += 120_000
        assertEquals(PairingState.Failed(PairingFailure.TIMED_OUT), window.sasFailed(PairingFailure.TIMED_OUT))
        assertFalse(window.claimSas())
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
