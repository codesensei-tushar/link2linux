package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.network.pairing.DesktopPeer
import dev.link2linux.android.core.network.pairing.PairingService
import dev.link2linux.android.core.network.tls.TestDesktop
import dev.link2linux.android.core.network.tls.asSource
import dev.link2linux.android.core.ring.RingEngine
import dev.link2linux.android.core.ring.RingNotifier
import dev.link2linux.android.core.ring.RingPlayer
import dev.link2linux.android.core.ring.RingVibrator
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.RingEndReason
import dev.link2linux.protocol.v1.RingRequest
import dev.link2linux.protocol.v1.RingStop
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

/**
 * The ring feature over real loopback TLS sessions, with a fake player,
 * vibrator and notifier behind the engine. The engine's own tests cover
 * its timing with the real minute; here the phone rings for [RING_MILLIS].
 */
class RingSessionTest {

    private class FakePlayer : RingPlayer {
        @Volatile var canRing = true

        @Volatile var playing = false

        override fun start(onInterrupted: () -> Unit): Boolean {
            if (!canRing) return false
            playing = true
            return true
        }

        override fun stop() {
            playing = false
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val player = FakePlayer()
    private val enabled = MutableStateFlow(true)
    private val engine = RingEngine(
        scope = scope,
        player = player,
        vibrator = object : RingVibrator {
            override fun start() = Unit

            override fun stop() = Unit
        },
        notifier = object : RingNotifier {
            override fun show(desktopName: String) = Unit

            override fun cancel() = Unit
        },
        enabled = enabled,
        durationMillis = RING_MILLIS,
    )
    private val devices = InMemoryPairedDeviceRepository()
    private val service = PairingService(
        identity = TestCertificates.generate("test phone").asSource(),
        devices = devices,
        localDevice = object : LocalDeviceRepository {
            override suspend fun localDevice() = Device(DeviceId("phone-1"), "Phone", DeviceKind.PHONE)
            override suspend fun setName(name: String?) = Unit
        },
        scope = scope,
        listenAddresses = ListenAddresses { local, _ -> local.isLoopbackAddress },
        hooks = RingSessionHooks(engine),
        port = 0,
        bindAddress = InetAddress.getLoopbackAddress(),
    )
    private val desktop = TestDesktop()

    @After
    fun tearDown() {
        runBlocking { service.stop() }
        scope.cancel()
    }

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    /** Pairs [desktop] in the store if needed and connects it, listing [features] in its Hello. */
    private suspend fun connect(
        features: List<String> = listOf("ring"),
        desktop: TestDesktop = this.desktop,
        deviceId: String = DesktopPeer.DEVICE_ID,
    ): DesktopPeer {
        val key = checkNotNull(desktop.fingerprint)
        if (devices.find(key) == null) {
            devices.save(
                PairedDevice(
                    device = Device(DeviceId(deviceId), "Desktop $deviceId", DeviceKind.DESKTOP),
                    fingerprint = key,
                    pairedAtEpochMillis = 0,
                    presenceKey = PresenceKey.of(ByteArray(32) { if (deviceId == DesktopPeer.DEVICE_ID) 9 else 10 }),
                ),
            )
        }
        service.start()
        val port = withTimeout(5_000) { service.listeningPort.filterNotNull().first() }
        return io {
            DesktopPeer(desktop.connect(port), desktop).apply {
                sendHello(PairingMethod.PAIRING_METHOD_UNSPECIFIED, deviceId, features)
                assertEquals(listOf("ring"), readHello().featuresList)
            }
        }
    }

    /** Reads `FeatureStatus` and `PresenceUpdate`, and returns what the status says of the feature. */
    private suspend fun DesktopPeer.readStart(): FeatureAvailability = io {
        val status = read()!!
        assertEquals(PayloadCase.PRESENCE_UPDATE, read()!!.payloadCase)
        status.availability()
    }

    private fun Envelope.availability(): FeatureAvailability {
        assertEquals(PayloadCase.FEATURE_STATUS, payloadCase)
        return featureStatus.featuresList.single { it.feature == "ring" }.availability
    }

    private suspend fun DesktopPeer.ring(): Envelope = io {
        val request = send {
            expectsReply = true
            ringRequest = RingRequest.getDefaultInstance()
        }
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    /** Sends `RingStop`, checks that the `Ack` comes next, and returns the `RingEnded` that follows if the phone rang. */
    private suspend fun DesktopPeer.stop(rang: Boolean): RingEndReason? = io {
        val request = send {
            expectsReply = true
            ringStop = RingStop.getDefaultInstance()
        }
        read()!!.also {
            assertEquals(request, it.replyTo)
            assertEquals(PayloadCase.ACK, it.payloadCase)
        }
        if (rang) read()!!.also { assertEquals(PayloadCase.RING_ENDED, it.payloadCase) }.ringEnded.reason else null
    }

    private suspend fun DesktopPeer.readEnded(): RingEndReason = io {
        read()!!.also { assertEquals(PayloadCase.RING_ENDED, it.payloadCase) }.ringEnded.reason
    }

    /** Pings the phone and checks that the `Pong` comes next: nothing else was sent before it. */
    private suspend fun DesktopPeer.assertNothingSent() = io {
        send { ping = ping { nonce = 7 } }
        assertEquals(PayloadCase.PONG, read()!!.payloadCase)
    }

    @Test
    fun theFeatureIsReadyUntilTheUserTurnsItOff() = runBlocking {
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())

        enabled.value = false

        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, io { peer.read()!! }.availability())
    }

    @Test
    fun aRequestRingsUntilTheDesktopStopsIt() = runBlocking {
        val peer = connect()
        peer.readStart()

        assertEquals(PayloadCase.ACK, peer.ring().payloadCase)
        assertTrue(player.playing)

        assertEquals(RingEndReason.RING_END_REASON_STOPPED_FROM_DESKTOP, peer.stop(rang = true))
        assertFalse(player.playing)
    }

    @Test
    fun withoutANotificationItRingsAllTheSameAndTheScreensStopEndsIt() = runBlocking {
        // The notifier here shows nothing, as when the user blocked the app's notifications.
        val peer = connect()
        peer.readStart()

        assertEquals(PayloadCase.ACK, peer.ring().payloadCase)
        assertTrue(player.playing)
        assertEquals("Test desktop", engine.ringingFor.value)

        // What the Ring screen's Stop button calls.
        engine.stopOnPhone()

        assertEquals(RingEndReason.RING_END_REASON_STOPPED_ON_PHONE, peer.readEnded())
        assertFalse(player.playing)
        assertEquals(null, engine.ringingFor.value)
    }

    @Test
    fun aStopWhileThePhoneDoesNotRingIsAcknowledged() = runBlocking {
        val peer = connect()
        peer.readStart()

        peer.stop(rang = false)
        peer.assertNothingSent()
    }

    @Test
    fun theDesktopIsToldWhenTheRingingTimesOut() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.ring()

        assertEquals(RingEndReason.RING_END_REASON_TIMED_OUT, peer.readEnded())
        assertFalse(player.playing)
    }

    @Test
    fun onlyTheDesktopsThatAskedAreToldThatItEnded() = runBlocking {
        val asking = connect()
        asking.readStart()
        val other = connect(desktop = TestDesktop(), deviceId = "desktop-2")
        other.readStart()

        asking.ring()
        asking.stop(rang = true)

        other.assertNothingSent()
    }

    @Test
    fun ringingOutlivesTheSessionAndTheReconnectedDesktopIsTold() = runBlocking {
        val first = connect()
        first.readStart()
        first.ring()

        io { first.close() }
        delay(RING_MILLIS / 4)
        assertTrue(player.playing)
        val again = connect()
        again.readStart()

        assertEquals(RingEndReason.RING_END_REASON_TIMED_OUT, again.readEnded())
    }

    @Test
    fun duringACallTheRequestIsUnavailable() = runBlocking {
        player.canRing = false
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, peer.ring().errorReply.code)
    }

    @Test
    fun whileTheFeatureIsOffTheRequestIsDisabled() = runBlocking {
        enabled.value = false
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, peer.ring().errorReply.code)
        assertFalse(player.playing)
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupported() = runBlocking {
        val peer = connect(features = listOf("notifications"))
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, peer.ring().errorReply.code)
        assertFalse(player.playing)
    }

    private companion object {
        // Long enough for a reconnect within it.
        const val RING_MILLIS = 2_000L
    }
}
