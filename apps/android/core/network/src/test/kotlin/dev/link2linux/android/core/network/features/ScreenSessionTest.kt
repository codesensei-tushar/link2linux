package dev.link2linux.android.core.network.features

import com.google.protobuf.ByteString
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
import dev.link2linux.android.core.screen.CaptureConfig
import dev.link2linux.android.core.screen.CaptureFormat
import dev.link2linux.android.core.screen.CaptureSink
import dev.link2linux.android.core.screen.ScreenAsker
import dev.link2linux.android.core.screen.ScreenCapture
import dev.link2linux.android.core.screen.ScreenController
import dev.link2linux.android.core.screen.ScreenEngine
import dev.link2linux.android.core.screen.ShareState
import dev.link2linux.android.core.screen.Stroke
import dev.link2linux.android.core.screen.TextField
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.ScreenCodec
import dev.link2linux.protocol.v1.ScreenEndReason
import dev.link2linux.protocol.v1.ScreenKey
import dev.link2linux.protocol.v1.ScreenKeyFrameRequest
import dev.link2linux.protocol.v1.ScreenStop
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.ping
import dev.link2linux.protocol.v1.screenGesture
import dev.link2linux.protocol.v1.screenInput
import dev.link2linux.protocol.v1.screenPoint
import dev.link2linux.protocol.v1.screenStart
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
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
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The screen feature over real loopback TLS sessions, with a fake asker,
 * capture and controller behind the engine. The engine's own tests cover
 * its rules; these check what goes over the wire, and in which order.
 */
class ScreenSessionTest {

    private class FakeAsker : ScreenAsker {
        @Volatile var canAsk = true
        val asked = CopyOnWriteArrayList<Long>()
        val withdrawn = CopyOnWriteArrayList<Long>()

        override fun ask(askId: Long, desktopName: String): Boolean {
            if (canAsk) asked += askId
            return canAsk
        }

        override fun withdraw(askId: Long) {
            withdrawn += askId
        }
    }

    private class FakeCapture : ScreenCapture {
        val started = CompletableDeferred<CaptureSink>()
        val stopped = CompletableDeferred<Unit>()
        @Volatile var keyFrameRequests = 0

        override fun start(config: CaptureConfig, sink: CaptureSink): Boolean {
            started.complete(sink)
            return true
        }

        override fun reconfigure(config: CaptureConfig) = Unit

        override fun requestKeyFrame() {
            keyFrameRequests++
        }

        override fun setQuality(quality: Float) = Unit

        override fun stop() {
            stopped.complete(Unit)
        }
    }

    private class FakeController : ScreenController {
        val done = CopyOnWriteArrayList<Any>()

        override suspend fun gesture(strokes: List<Stroke>): Boolean {
            done.add(strokes)
            return true
        }

        override fun navigate(key: ScreenKey): Boolean {
            done.add(key)
            return true
        }

        override fun focusedField(): TextField? = null
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val asker = FakeAsker()
    private val controller = FakeController()
    private val enabled = MutableStateFlow(true)
    private val control = MutableStateFlow(false)
    private val engine = ScreenEngine(
        scope = scope,
        asker = asker,
        encodable = setOf(ScreenCodec.SCREEN_CODEC_H264, ScreenCodec.SCREEN_CODEC_JPEG),
        controller = controller,
        enabled = enabled,
        control = control,
        isLocked = { false },
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
        hooks = ScreenSessionHooks(engine),
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
        features: List<String> = listOf("screen"),
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
                assertEquals(listOf("screen"), readHello().featuresList)
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
        return featureStatus.featuresList.single { it.feature == "screen" }.availability
    }

    private suspend fun DesktopPeer.request(fill: Envelope.Builder.() -> Unit): Envelope = io {
        val request = send {
            expectsReply = true
            fill()
        }
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    private suspend fun DesktopPeer.start(): Envelope = request {
        screenStart = screenStart {
            codecs += ScreenCodec.SCREEN_CODEC_H264
            maxSize = 1200
        }
    }

    private suspend fun DesktopPeer.next(): Envelope = io { read()!! }

    /** Pings the phone and checks that the `Pong` comes next: nothing else was sent before it. */
    private suspend fun DesktopPeer.assertNothingSent() = io {
        send { ping = ping { nonce = 7 } }
        assertEquals(PayloadCase.PONG, read()!!.payloadCase)
    }

    /** Starts a stream to [peer] the user agrees to, and reads its first format. */
    private suspend fun stream(peer: DesktopPeer): Pair<FakeCapture, CaptureSink> {
        assertEquals(PayloadCase.ACK, peer.start().payloadCase)
        val capture = FakeCapture()
        engine.granted(asker.asked.last(), capture)
        val sink = withTimeout(5_000) { capture.started.await() }
        sink.onFormat(CaptureFormat(ScreenCodec.SCREEN_CODEC_H264, 540, 1200, ByteString.copyFrom(byteArrayOf(1, 66, 0, 31)), 1080, 2400))
        return capture to sink
    }

    @Test
    fun theFeatureIsReadyWhileTheUserHasItOn() = runBlocking {
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())

        enabled.value = false

        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, peer.next().availability())
    }

    @Test
    fun theStreamComesAsItsFormatThenItsFramesAndStopsWithTheDesktopsStop() = runBlocking {
        val peer = connect()
        peer.readStart()
        val (capture, sink) = stream(peer)

        val format = peer.next()
        sink.onFrame(1_000, true, ByteArray(10) { 1 })
        sink.onFrame(17_000, false, ByteArray(10) { 2 })
        val frames = List(2) { peer.next() }

        assertEquals(PayloadCase.SCREEN_FORMAT, format.payloadCase)
        assertEquals(540, format.screenFormat.width)
        assertEquals(1200, format.screenFormat.height)
        assertEquals(listOf(PayloadCase.SCREEN_FRAME, PayloadCase.SCREEN_FRAME), frames.map { it.payloadCase })
        assertEquals(listOf(1_000L, 17_000L), frames.map { it.screenFrame.timestampUs })
        assertTrue(frames[0].screenFrame.keyFrame)

        assertEquals(PayloadCase.ACK, peer.request { screenStop = ScreenStop.getDefaultInstance() }.payloadCase)
        withTimeout(5_000) { capture.stopped.await() }
        sink.onFrame(33_000, false, ByteArray(10))
        peer.assertNothingSent()
    }

    @Test
    fun aFrameOfTheLargestSizeGoesThrough() = runBlocking {
        val peer = connect()
        peer.readStart()
        val (_, sink) = stream(peer)
        peer.next()

        sink.onFrame(0, true, ByteArray(ScreenEngine.MAX_FRAME_BYTES) { it.toByte() })

        assertEquals(ScreenEngine.MAX_FRAME_BYTES, peer.next().screenFrame.data.size())
    }

    @Test
    fun anotherDesktopIsBusyWhileTheScreenGoesToTheFirst() = runBlocking {
        val first = connect()
        first.readStart()
        stream(first)
        val other = connect(desktop = TestDesktop(), deviceId = "desktop-2")
        other.readStart()

        assertEquals(ErrorCode.ERROR_CODE_BUSY, other.start().errorReply.code)
    }

    @Test
    fun theDesktopIsToldWhenTheUserDeclines() = runBlocking {
        val peer = connect()
        peer.readStart()
        assertEquals(PayloadCase.ACK, peer.start().payloadCase)

        engine.declined(asker.asked.last())

        assertEquals(ScreenEndReason.SCREEN_END_REASON_DECLINED, peer.next().screenEnded.reason)
    }

    @Test
    fun aPhoneThatCannotAskAnswersNotAllowed() = runBlocking {
        asker.canAsk = false
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, peer.start().errorReply.code)
    }

    @Test
    fun withControlOnTheFormatSaysSoAndInputIsCarriedOut() = runBlocking {
        control.value = true
        val peer = connect()
        peer.readStart()
        stream(peer)
        val format = peer.next().screenFormat
        assertTrue(format.input)

        val tap = peer.request {
            screenInput = screenInput {
                gesture = screenGesture {
                    formatId = format.id
                    points += screenPoint {
                        x = 10
                        y = 20
                    }
                }
            }
        }
        val home = peer.request { screenInput = screenInput { key = ScreenKey.SCREEN_KEY_HOME } }

        assertEquals(PayloadCase.ACK, tap.payloadCase)
        assertEquals(PayloadCase.ACK, home.payloadCase)
        assertEquals(ScreenKey.SCREEN_KEY_HOME, controller.done.last())

        control.value = false
        val changed = peer.next().screenFormat
        assertEquals(format.id, changed.id)
        assertFalse(changed.input)
        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, peer.request { screenInput = screenInput { key = ScreenKey.SCREEN_KEY_BACK } }.errorReply.code)
    }

    @Test
    fun aKeyFrameRequestReachesTheCapture() = runBlocking {
        val peer = connect()
        peer.readStart()
        val (capture, _) = stream(peer)
        peer.next()

        io { peer.send { screenKeyFrameRequest = ScreenKeyFrameRequest.getDefaultInstance() } }
        peer.assertNothingSent()

        assertEquals(1, capture.keyFrameRequests)
    }

    @Test
    fun theDesktopLeavingReadyEndsTheStream() = runBlocking {
        val peer = connect()
        peer.readStart()
        val (capture, _) = stream(peer)
        peer.next()

        io {
            peer.send {
                featureStatus = featureStatus {
                    features += featureState {
                        feature = "screen"
                        availability = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
                    }
                }
            }
        }

        assertEquals(ScreenEndReason.SCREEN_END_REASON_INTERRUPTED, peer.next().screenEnded.reason)
        withTimeout(5_000) { capture.stopped.await() }
    }

    @Test
    fun theStreamEndsWithItsSession() = runBlocking {
        val peer = connect()
        peer.readStart()
        val (capture, _) = stream(peer)

        io { peer.close() }

        withTimeout(5_000) { capture.stopped.await() }
        // The phone shares with nobody now, so another desktop may ask.
        withTimeout(5_000) { engine.share.first { it == ShareState.Idle } }
        val other = connect(desktop = TestDesktop(), deviceId = "desktop-2")
        other.readStart()
        assertEquals(PayloadCase.ACK, other.start().payloadCase)
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupported() = runBlocking {
        val peer = connect(features = listOf("notifications"))
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, peer.start().errorReply.code)
        assertTrue(asker.asked.isEmpty())
    }
}
