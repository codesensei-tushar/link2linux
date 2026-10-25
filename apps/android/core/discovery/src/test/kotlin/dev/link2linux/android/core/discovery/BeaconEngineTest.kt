package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.QrPairingOffer
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress

@OptIn(ExperimentalCoroutinesApi::class)
class BeaconEngineTest {

    private class Sent(val timeMillis: Long, val flagsByte: Int, val unicast: List<InetSocketAddress>, val keepAwake: Boolean)

    private class FakeTransport(private val clock: () -> Long) : BeaconTransport {
        val bursts = mutableListOf<Pair<Long, BurstKind>>()
        val sent = mutableListOf<Sent>()
        override fun onBurstStart(kind: BurstKind) {
            bursts += clock() to kind
        }
        override suspend fun send(datagram: ByteArray, unicast: List<InetSocketAddress>, keepAwake: Boolean) {
            sent += Sent(clock(), datagram[5].toInt(), unicast, keepAwake)
        }
    }

    private val a = BeaconDesktop(KeyFingerprint.of(ByteArray(32) { 1 }), PresenceKey.of(ByteArray(32) { 1 }), listOf(addr(1)))
    private val b = BeaconDesktop(KeyFingerprint.of(ByteArray(32) { 2 }), PresenceKey.of(ByteArray(32) { 2 }), listOf(addr(2)))
    private val offer = QrPairingOffer(a.fingerprint, PairingSecret.of(ByteArray(16)), "Desk", listOf(addr(9)))

    private fun addr(n: Int) = InetSocketAddress(InetAddress.getByAddress(byteArrayOf(10, 0, 0, n.toByte())), 27381)

    private val random = RandomBytes { size -> ByteArray(size) { 7 } }

    private fun TestScope.engine(): Pair<BeaconEngine, FakeTransport> {
        val transport = FakeTransport { currentTime }
        return BeaconEngine(backgroundScope, transport, random) { 1_000_000 + currentTime } to transport
    }

    private val ready = BeaconInputs(port = 27380, networkAvailable = true)

    @Test
    fun scheduleShapes() {
        assertEquals(120, BurstSchedule.qrWait.size)
        assertEquals(119_000, BurstSchedule.qrWait.last())
        val s = BurstSchedule.pairedNoSession
        assertEquals((0..9).map { it * 1000L }, s.take(10))
        assertEquals(listOf(10_000L, 15_000L), s.slice(10..11))
        assertEquals(60_000L, s[20])
        assertEquals(90_000L, s[21])
        assertEquals(600_000L, s.last())
        assertTrue(BurstSchedule.holdsWakeLock(59_999))
        assertTrue(!BurstSchedule.holdsWakeLock(60_000))
    }

    @Test
    fun pairedPhoneWithoutSessionSendsOneBeaconPerDesktopAtEachStep() = runTest {
        val (engine, transport) = engine()
        engine.update(ready.copy(desktops = listOf(a, b)))
        runCurrent()
        assertEquals(listOf(0L to BurstKind.PRESENCE), transport.bursts)
        assertEquals(2, transport.sent.size)
        assertEquals(listOf(listOf(addr(1)), listOf(addr(2))), transport.sent.map { it.unicast })
        assertTrue(transport.sent.all { it.flagsByte == 0 })

        advanceTimeBy(600_000)
        runCurrent()
        assertEquals(BurstSchedule.pairedNoSession.size * 2, transport.sent.size)
        assertEquals(transport.sent.last().timeMillis, 600_000)
        // Beyond the first minute the device is no longer kept awake for them.
        assertTrue(transport.sent.filter { it.timeMillis < 60_000 }.all { it.keepAwake })
        assertTrue(transport.sent.filter { it.timeMillis >= 60_000 }.none { it.keepAwake })
    }

    @Test
    fun burstStopsWhenEverySessionIsUp() = runTest {
        val (engine, transport) = engine()
        val inputs = ready.copy(desktops = listOf(a))
        engine.update(inputs)
        advanceTimeBy(2_500)
        val before = transport.sent.size
        assertEquals(3, before)
        engine.update(inputs.copy(connected = setOf(a.fingerprint)))
        advanceTimeBy(60_000)
        assertEquals(before, transport.sent.size)
    }

    @Test
    fun skipsDesktopsThatHaveASession() = runTest {
        val (engine, transport) = engine()
        engine.update(ready.copy(desktops = listOf(a, b), connected = setOf(a.fingerprint)))
        runCurrent()
        assertEquals(listOf(listOf(addr(2))), transport.sent.map { it.unicast })
    }

    @Test
    fun lostSessionStartsAFreshBurst() = runTest {
        val (engine, transport) = engine()
        val up = ready.copy(desktops = listOf(a), connected = setOf(a.fingerprint))
        engine.update(up)
        advanceTimeBy(5_000)
        assertTrue(transport.bursts.isEmpty())
        engine.update(up.copy(connected = emptySet()))
        runCurrent()
        assertEquals(listOf(5_000L to BurstKind.PRESENCE), transport.bursts)
        // Updates that change nothing relevant do not restart it.
        engine.update(up.copy(connected = emptySet()))
        runCurrent()
        assertEquals(1, transport.bursts.size)
    }

    @Test
    fun noBeaconsWithoutNetworkOrPort() = runTest {
        val (engine, transport) = engine()
        engine.update(BeaconInputs(port = null, networkAvailable = true, desktops = listOf(a)))
        engine.update(BeaconInputs(port = 27380, networkAvailable = false, desktops = listOf(a)))
        advanceTimeBy(10_000)
        assertTrue(transport.sent.isEmpty())
        engine.update(ready.copy(desktops = listOf(a)))
        runCurrent()
        assertEquals(1, transport.sent.size) // joining a network starts the burst
    }

    @Test
    fun triggerRestartsThePresenceBurst() = runTest {
        val (engine, transport) = engine()
        engine.update(ready.copy(desktops = listOf(a)))
        advanceTimeBy(700_000)
        val before = transport.sent.size
        engine.trigger()
        runCurrent()
        assertEquals(before + 1, transport.sent.size)
        assertEquals(2, transport.bursts.size)
    }

    @Test
    fun qrWaitSendsPairingBeaconsToTheOffersAddressesForTwoMinutes() = runTest {
        val (engine, transport) = engine()
        engine.update(ready.copy(qrOffer = offer, qrUntilMillis = 1_000_000 + 300_000))
        advanceTimeBy(130_000)
        assertEquals(listOf(0L to BurstKind.PAIRING), transport.bursts)
        assertEquals(120, transport.sent.size)
        assertTrue(transport.sent.all { it.flagsByte == 1 && it.unicast == listOf(addr(9)) })
    }

    @Test
    fun qrBurstEndsWhenTheOfferExpires() = runTest {
        val (engine, transport) = engine()
        engine.update(ready.copy(qrOffer = offer, qrUntilMillis = 1_000_000 + 3_500))
        advanceTimeBy(20_000)
        assertEquals(4, transport.sent.size)
    }

    @Test
    fun pairedAndWaitingSendBothKindsOnTheirOwnSchedules() = runTest {
        val (engine, transport) = engine()
        engine.update(ready.copy(desktops = listOf(b), qrOffer = offer, qrUntilMillis = 1_000_000 + 300_000))
        runCurrent()
        assertEquals(setOf(BurstKind.PAIRING, BurstKind.PRESENCE), transport.bursts.map { it.second }.toSet())
        assertEquals(setOf(0, 1), transport.sent.map { it.flagsByte }.toSet())
        // A repeated update with the same offer does not restart the pairing burst.
        engine.update(ready.copy(desktops = listOf(b), qrOffer = offer, qrUntilMillis = 1_000_000 + 300_000))
        runCurrent()
        assertEquals(2, transport.bursts.size)
    }

    @Test
    fun announcerKeepsNameAndNonceInSasMode() {
        var n = 0
        val announcer = Announcer { size -> ByteArray(size) { (n++).toByte() } }
        val first = announcer.announcement(AnnounceState(sasMode = true))
        announcer.rotate(sasMode = true)
        assertEquals(first, announcer.announcement(AnnounceState(sasMode = true)))
        announcer.rotate()
        val key = PresenceKey.of(ByteArray(32))
        assertTrue(first.instanceName != announcer.announcement(AnnounceState(listOf(key))).instanceName)
    }
}
