package dev.link2linux.android.core.screen

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ScreenCodec
import dev.link2linux.protocol.v1.ScreenEndReason
import dev.link2linux.protocol.v1.ScreenFormat
import dev.link2linux.protocol.v1.ScreenFrame
import dev.link2linux.protocol.v1.ScreenInput
import dev.link2linux.protocol.v1.ScreenKey
import dev.link2linux.protocol.v1.ScreenStart
import dev.link2linux.protocol.v1.screenGesture
import dev.link2linux.protocol.v1.screenInput
import dev.link2linux.protocol.v1.screenPoint
import dev.link2linux.protocol.v1.screenStart
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ScreenEngineTest {

    private class FakePeer(override val desktopId: String, override val desktopName: String = "Desk $desktopId") : ScreenPeer {
        /** What went out, in order: formats, frames and end reasons. */
        val sent = mutableListOf<Any>()

        /** While set, writing a frame waits for it, as on a slow network. */
        var network: CompletableDeferred<Unit>? = null

        override suspend fun format(format: ScreenFormat) {
            sent += format
        }

        override suspend fun frame(frame: ScreenFrame) {
            network?.await()
            sent += frame
        }

        override suspend fun ended(reason: ScreenEndReason) {
            sent += reason
        }

        val formats get() = sent.filterIsInstance<ScreenFormat>()
        val frames get() = sent.filterIsInstance<ScreenFrame>()
        val ends get() = sent.filterIsInstance<ScreenEndReason>()
    }

    private class FakeAsker : ScreenAsker {
        var canAsk = true
        val asked = mutableListOf<Pair<Long, String>>()
        val withdrawn = mutableListOf<Long>()

        override fun ask(askId: Long, desktopName: String): Boolean {
            if (canAsk) asked += askId to desktopName
            return canAsk
        }

        override fun withdraw(askId: Long) {
            withdrawn += askId
        }

        val lastId: Long get() = asked.last().first
    }

    private class FakeCapture(private val startsFine: Boolean = true) : ScreenCapture {
        var config: CaptureConfig? = null
        lateinit var sink: CaptureSink
        val reconfigured = mutableListOf<CaptureConfig>()
        var keyFrameRequests = 0
        val qualities = mutableListOf<Float>()
        var stopped = false

        override fun start(config: CaptureConfig, sink: CaptureSink): Boolean {
            this.config = config
            this.sink = sink
            return startsFine
        }

        override fun reconfigure(config: CaptureConfig) {
            reconfigured += config
        }

        override fun requestKeyFrame() {
            keyFrameRequests++
        }

        override fun setQuality(quality: Float) {
            qualities += quality
        }

        override fun stop() {
            stopped = true
        }

        fun format(width: Int = 540, height: Int = 1200, codec: ScreenCodec = ScreenCodec.SCREEN_CODEC_H264) =
            sink.onFormat(CaptureFormat(codec, width, height, ByteString.copyFrom(byteArrayOf(1, 2, 3)), width * 2, height * 2))

        fun frame(atMs: Long, key: Boolean = false, size: Int = 100) = sink.onFrame(atMs * 1000, key, ByteArray(size))
    }

    private val asker = FakeAsker()
    private val controller = FakeController()
    private val enabled = MutableStateFlow(true)
    private val control = MutableStateFlow(false)
    private var locked = false

    private fun TestScope.engine(encodable: Set<ScreenCodec> = setOf(ScreenCodec.SCREEN_CODEC_H264, ScreenCodec.SCREEN_CODEC_JPEG)) =
        ScreenEngine(backgroundScope, asker, encodable, controller, enabled, control, isLocked = { locked })

    private fun request(codecs: List<ScreenCodec> = listOf(ScreenCodec.SCREEN_CODEC_H264), maxSize: Int = 0): ScreenStart = screenStart {
        this.codecs.addAll(codecs)
        this.maxSize = maxSize
    }

    private suspend fun ScreenEngine.ask(peer: FakePeer, request: ScreenStart = request()): ScreenReply {
        var reply: ScreenReply? = null
        start(peer, request) { reply = it }
        return checkNotNull(reply)
    }

    private suspend fun ScreenEngine.stopFrom(peer: FakePeer): ScreenReply {
        var reply: ScreenReply? = null
        stop(peer) { reply = it }
        return checkNotNull(reply)
    }

    /** Has [peer] ask, the user agree and the capture report its format; returns the capture. */
    private suspend fun TestScope.share(engine: ScreenEngine, peer: FakePeer, request: ScreenStart = request()): FakeCapture {
        assertEquals(ScreenReply.Done, engine.ask(peer, request))
        val capture = FakeCapture()
        engine.granted(asker.lastId, capture)
        runCurrent()
        capture.format()
        runCurrent()
        return capture
    }

    private fun tap(formatId: Int, x: Int = 1, y: Int = 1): ScreenInput = screenInput {
        gesture = screenGesture {
            this.formatId = formatId
            points += screenPoint {
                this.x = x
                this.y = y
            }
        }
    }

    private fun ScreenReply?.code(): ErrorCode? = (this as? ScreenReply.Refused)?.code

    @Test
    fun aStartAsksTheUserNamingTheDesktopThenIsAcknowledged() = runTest {
        val engine = engine()
        val peer = FakePeer("a")

        assertEquals(ScreenReply.Done, engine.ask(peer))

        assertEquals(listOf(1L to "Desk a"), asker.asked)
        assertEquals(ShareState.Asking("Desk a"), engine.share.value)
        assertTrue(engine.isAsking(1))
    }

    @Test
    fun onceTheUserAgreesTheFormatThenTheFramesGoOut() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer, request(listOf(ScreenCodec.SCREEN_CODEC_H264), maxSize = 1200))

        capture.frame(0, key = true)
        capture.frame(16)
        runCurrent()

        assertEquals(CaptureConfig(ScreenCodec.SCREEN_CODEC_H264, 1200), capture.config)
        assertEquals(listOf(1L), asker.withdrawn)
        assertEquals(ShareState.Sharing("Desk a"), engine.share.value)
        val format = peer.formats.single()
        assertEquals(ScreenCodec.SCREEN_CODEC_H264, format.codec)
        assertEquals(540, format.width)
        assertEquals(1200, format.height)
        assertEquals(ByteString.copyFrom(byteArrayOf(1, 2, 3)), format.codecConfig)
        assertFalse(format.input)
        assertTrue(peer.sent[0] is ScreenFormat)
        assertEquals(listOf(0L to true, 16_000L to false), peer.frames.map { it.timestampUs to it.keyFrame })
    }

    @Test
    fun theFirstCodecThePhoneCanEncodeIsTaken() = runTest {
        val engine = engine(encodable = setOf(ScreenCodec.SCREEN_CODEC_JPEG))
        val capture = share(engine, FakePeer("a"), request(listOf(ScreenCodec.SCREEN_CODEC_H264, ScreenCodec.SCREEN_CODEC_JPEG)))

        assertEquals(ScreenCodec.SCREEN_CODEC_JPEG, capture.config?.codec)
    }

    @Test
    fun aStartOutOfRangeIsInvalid() = runTest {
        val engine = engine(encodable = setOf(ScreenCodec.SCREEN_CODEC_JPEG))
        val peer = FakePeer("a")

        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.ask(peer, request(emptyList())).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.ask(peer, request(List(9) { ScreenCodec.SCREEN_CODEC_JPEG })).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_H264))).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = 239)).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = 4097)).code())
        // A uint32 above Int.MAX_VALUE reads as negative in Java.
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = -1)).code())
        assertTrue(asker.asked.isEmpty())

        assertEquals(ScreenReply.Done, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = 240)))
        assertEquals(ScreenReply.Done, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = 4096)))
    }

    @Test
    fun whileTheFeatureIsOffTheStartIsDisabled() = runTest {
        enabled.value = false
        val engine = engine()

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, engine.ask(FakePeer("a")).code())
        assertTrue(asker.asked.isEmpty())
    }

    @Test
    fun aPhoneThatCannotAskAnswersNotAllowed() = runTest {
        asker.canAsk = false
        val engine = engine()

        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, engine.ask(FakePeer("a")).code())
        assertEquals(ShareState.Idle, engine.share.value)
    }

    @Test
    fun anotherDesktopIsBusyWhileThePhoneAsksAndWhileItShares() = runTest {
        val engine = engine()
        val first = FakePeer("a")
        val other = FakePeer("b")

        engine.ask(first)
        assertEquals(ErrorCode.ERROR_CODE_BUSY, engine.ask(other).code())

        engine.granted(asker.lastId, FakeCapture())
        runCurrent()
        assertEquals(ErrorCode.ERROR_CODE_BUSY, engine.ask(other).code())
        assertEquals(1, asker.asked.size)
    }

    @Test
    fun theSameDesktopsNewSessionTakesThePlaceOfItsOldOne() = runTest {
        val engine = engine()
        val old = FakePeer("a")
        val capture = share(engine, old)
        val again = FakePeer("a")

        assertEquals(ScreenReply.Done, engine.ask(again))
        runCurrent()

        assertTrue(capture.stopped)
        assertTrue(old.ends.isEmpty())
        assertEquals(ShareState.Asking("Desk a"), engine.share.value)
        assertEquals(2, asker.asked.size)
    }

    @Test
    fun theUserDecliningEndsTheStreamThenTheDesktopMayAskAgain() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        engine.ask(peer)

        engine.declined(asker.lastId)
        runCurrent()

        assertEquals(listOf(ScreenEndReason.SCREEN_END_REASON_DECLINED), peer.ends)
        assertEquals(listOf(1L), asker.withdrawn)
        assertEquals(ShareState.Idle, engine.share.value)
        assertEquals(ScreenReply.Done, engine.ask(peer))
        assertEquals(2, asker.asked.size)
    }

    @Test
    fun withoutAnAnswerInAMinuteTheQuestionTimesOut() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        engine.ask(peer)

        advanceTimeBy(ScreenEngine.ANSWER_MILLIS - 1)
        runCurrent()
        assertTrue(peer.ends.isEmpty())
        advanceTimeBy(2)
        runCurrent()

        assertEquals(listOf(ScreenEndReason.SCREEN_END_REASON_TIMED_OUT), peer.ends)
        assertEquals(listOf(1L), asker.withdrawn)
        assertFalse(engine.isAsking(1))
    }

    @Test
    fun aConsentThatComesTooLateStopsItsCapture() = runTest {
        val engine = engine()
        engine.ask(FakePeer("a"))
        advanceTimeBy(ScreenEngine.ANSWER_MILLIS + 1)
        val late = FakeCapture()

        engine.granted(1, late)
        runCurrent()

        assertTrue(late.stopped)
        assertEquals(ShareState.Idle, engine.share.value)
    }

    @Test
    fun aRepeatedStartWhileSharingIsAcknowledgedWithoutAskingAndRestartsTheVideo() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)
        capture.frame(0, key = true)
        runCurrent()

        assertEquals(ScreenReply.Done, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = 720)))
        assertEquals(listOf(CaptureConfig(ScreenCodec.SCREEN_CODEC_JPEG, 720)), capture.reconfigured)
        // The capture does as asked: a new format, then a key frame.
        capture.format(width = 324, height = 720, codec = ScreenCodec.SCREEN_CODEC_JPEG)
        capture.frame(30, key = true)
        runCurrent()

        assertEquals(1, asker.asked.size)
        val (first, second) = peer.formats
        assertNotEquals(first.id, second.id)
        assertEquals(ScreenCodec.SCREEN_CODEC_JPEG, second.codec)
        assertEquals(listOf(true, true), peer.frames.map { it.keyFrame })
    }

    @Test
    fun aRepeatedStartWhileAskingKeepsAskingWithTheNewRequest() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        engine.ask(peer)

        assertEquals(ScreenReply.Done, engine.ask(peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG), maxSize = 800)))
        val capture = FakeCapture()
        engine.granted(asker.lastId, capture)
        runCurrent()

        assertEquals(1, asker.asked.size)
        assertEquals(CaptureConfig(ScreenCodec.SCREEN_CODEC_JPEG, 800), capture.config)
    }

    @Test
    fun theDesktopsStopEndsTheStreamWithoutScreenEndedAndIsAcknowledgedAfterItsLastFrame() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)
        capture.frame(0, key = true)
        runCurrent()

        assertEquals(ScreenReply.Done, engine.stopFrom(peer))
        capture.frame(16)
        runCurrent()

        assertTrue(capture.stopped)
        assertTrue(peer.ends.isEmpty())
        assertEquals(1, peer.frames.size)
        assertEquals(ShareState.Idle, engine.share.value)
    }

    @Test
    fun aStopWhileThePhoneAsksTakesTheQuestionBack() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        engine.ask(peer)

        assertEquals(ScreenReply.Done, engine.stopFrom(peer))
        runCurrent()

        assertEquals(listOf(1L), asker.withdrawn)
        assertTrue(peer.ends.isEmpty())
    }

    @Test
    fun aStopFromADesktopThatDoesNotShareIsAcknowledgedAndChangesNothing() = runTest {
        val engine = engine()
        val capture = share(engine, FakePeer("a"))

        assertEquals(ScreenReply.Done, engine.stopFrom(FakePeer("b")))

        assertFalse(capture.stopped)
    }

    @Test
    fun theUserStoppingOnThePhoneEndsTheStream() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        engine.stopOnPhone()
        runCurrent()

        assertTrue(capture.stopped)
        assertEquals(listOf(ScreenEndReason.SCREEN_END_REASON_STOPPED_ON_PHONE), peer.ends)
    }

    @Test
    fun aCaptureThatStopsByItselfEndsTheStreamAsLockedWhileThePhoneIsLocked() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        locked = true
        capture.sink.onStopped()
        runCurrent()

        assertEquals(listOf(ScreenEndReason.SCREEN_END_REASON_LOCKED), peer.ends)
    }

    @Test
    fun aCaptureThatStopsByItselfWhileUnlockedWasStoppedOnThePhone() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        capture.sink.onStopped()
        runCurrent()

        assertEquals(listOf(ScreenEndReason.SCREEN_END_REASON_STOPPED_ON_PHONE), peer.ends)
    }

    @Test
    fun aFailingOrUnstartableCaptureInterrupts() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        share(engine, peer).sink.onFailed()
        runCurrent()

        engine.ask(peer)
        val broken = FakeCapture(startsFine = false)
        engine.granted(asker.lastId, broken)
        runCurrent()

        engine.ask(peer)
        engine.couldNotStart(asker.lastId)
        runCurrent()

        assertEquals(List(3) { ScreenEndReason.SCREEN_END_REASON_INTERRUPTED }, peer.ends)
        assertTrue(broken.stopped)
    }

    @Test
    fun turningTheFeatureOffInterruptsTheStreamAndTheQuestion() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        enabled.value = false
        runCurrent()
        assertTrue(capture.stopped)

        enabled.value = true
        runCurrent()
        engine.ask(peer)
        enabled.value = false
        runCurrent()

        assertEquals(List(2) { ScreenEndReason.SCREEN_END_REASON_INTERRUPTED }, peer.ends)
        assertEquals(listOf(1L, 2L), asker.withdrawn)
    }

    @Test
    fun theDesktopLeavingReadyInterruptsItsStream() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        engine.notReadyOn(FakePeer("b"))
        assertFalse(capture.stopped)
        engine.notReadyOn(peer)
        runCurrent()

        assertTrue(capture.stopped)
        assertEquals(listOf(ScreenEndReason.SCREEN_END_REASON_INTERRUPTED), peer.ends)
    }

    @Test
    fun theStreamEndsWithItsSessionWithoutAMessage() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        engine.sessionEnded(peer)
        runCurrent()

        assertTrue(capture.stopped)
        assertTrue(peer.ends.isEmpty())
        assertEquals(ShareState.Idle, engine.share.value)
    }

    @Test
    fun theFormatSaysWhetherControlIsOnAndChangesKeepItsId() = runTest {
        control.value = true
        val engine = engine()
        val peer = FakePeer("a")
        share(engine, peer)

        control.value = false
        runCurrent()
        control.value = true
        runCurrent()

        assertEquals(listOf(true, false, true), peer.formats.map { it.input })
        assertEquals(1, peer.formats.map { it.id }.distinct().size)
    }

    @Test
    fun inputIsCarriedOutOnlyWithControlOnAndOnlyForTheDesktopTheScreenGoesTo() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        share(engine, peer)
        val id = peer.formats.single().id
        val replies = mutableListOf<ScreenReply>()

        engine.input(peer, tap(id), replies::add)
        control.value = true
        runCurrent()
        engine.input(FakePeer("b"), tap(id), replies::add)
        engine.input(peer, tap(id), replies::add)
        runCurrent()

        assertEquals(listOf(ErrorCode.ERROR_CODE_NOT_ALLOWED, ErrorCode.ERROR_CODE_NOT_ALLOWED, null), replies.map { it.code() })
        assertEquals(1, controller.done.size)
    }

    @Test
    fun inputBeforeTheUserAgreedIsNotAllowed() = runTest {
        control.value = true
        val engine = engine()
        val peer = FakePeer("a")
        engine.ask(peer)
        var reply: ScreenReply? = null

        engine.input(peer, screenInput { key = ScreenKey.SCREEN_KEY_HOME }) { reply = it }

        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, reply.code())
    }

    @Test
    fun inputsAreCarriedOutOneAfterTheOtherInTheirOrder() = runTest {
        control.value = true
        val engine = engine()
        val peer = FakePeer("a")
        share(engine, peer)
        val id = peer.formats.single().id
        val gesture = CompletableDeferred<Boolean>().also { controller.gestureEnd = it }
        val replies = mutableListOf<String>()

        engine.input(peer, tap(id)) { replies += "tap" }
        engine.input(peer, screenInput { key = ScreenKey.SCREEN_KEY_BACK }) { replies += "back" }
        runCurrent()
        // The key waits for the tap to finish.
        assertEquals(1, controller.done.size)
        assertTrue(replies.isEmpty())

        gesture.complete(true)
        runCurrent()

        assertEquals(listOf("tap", "back"), replies)
        assertEquals(ScreenKey.SCREEN_KEY_BACK, controller.done[1])
    }

    @Test
    fun aPasswordTypedDuringAStreamIsForgottenWithIt() = runTest {
        control.value = true
        val engine = engine()
        val peer = FakePeer("a")
        val password = FakeField("", isPassword = true).also { controller.field = it }
        val replies = mutableListOf<ScreenReply>()

        share(engine, peer)
        engine.input(peer, screenInput { text = "ab" }, replies::add)
        engine.input(peer, screenInput { text = "c" }, replies::add)
        runCurrent()
        engine.stopFrom(peer)
        share(engine, peer)
        engine.input(peer, screenInput { text = "d" }, replies::add)
        runCurrent()

        assertEquals(listOf(null, null, ErrorCode.ERROR_CODE_NOT_ALLOWED), replies.map { it.code() })
        assertEquals("abc", password.content)
    }

    @Test
    fun aGestureOnFramesOfAnOlderFormatHasExpired() = runTest {
        control.value = true
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)
        val old = peer.formats.single().id
        capture.format(width = 1200, height = 540)
        runCurrent()
        var reply: ScreenReply? = null

        engine.input(peer, tap(old)) { reply = it }
        runCurrent()

        assertEquals(ErrorCode.ERROR_CODE_EXPIRED, reply.code())
    }

    @Test
    fun tooManyWaitingInputsAreBusyAndWaitingOnesAreRefusedWhenTheStreamEnds() = runTest {
        control.value = true
        val engine = engine()
        val peer = FakePeer("a")
        share(engine, peer)
        val id = peer.formats.single().id
        controller.gestureEnd = CompletableDeferred()
        val replies = mutableListOf<ScreenReply>()

        // One being carried out, then the most that may wait, then one more.
        repeat(ScreenEngine.MAX_WAITING_INPUTS + 2) {
            engine.input(peer, tap(id), replies::add)
            runCurrent()
        }
        assertEquals(listOf(ErrorCode.ERROR_CODE_BUSY), replies.map { it.code() })

        engine.stopFrom(peer)
        runCurrent()

        assertEquals(ScreenEngine.MAX_WAITING_INPUTS + 2, replies.size)
        assertTrue(replies.drop(1).all { it.code() == ErrorCode.ERROR_CODE_NOT_ALLOWED })
    }

    @Test
    fun aFrameTooLargeIsDroppedWithTheFramesUpToTheNextKeyFrameAndTheQualityDrops() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)
        capture.frame(0, key = true)
        runCurrent()

        capture.frame(16, size = ScreenEngine.MAX_FRAME_BYTES + 1)
        capture.frame(33)
        capture.frame(50, key = true, size = ScreenEngine.MAX_FRAME_BYTES)
        runCurrent()

        assertEquals(listOf(0L, 50_000L), peer.frames.map { it.timestampUs })
        assertEquals(1, capture.keyFrameRequests)
        assertEquals(listOf(0.7f), capture.qualities)
    }

    @Test
    fun aSlowNetworkDropsFramesAsksForAKeyFrameAndLowersTheQuality() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)
        val network = CompletableDeferred<Unit>().also { peer.network = it }

        capture.frame(0, key = true)
        runCurrent()
        capture.frame(400)
        capture.frame(501)
        capture.frame(520)
        assertEquals(1, capture.keyFrameRequests)
        assertEquals(listOf(0.7f), capture.qualities)
        capture.frame(600, key = true)
        peer.network = null
        network.complete(Unit)
        runCurrent()

        // The frame being written when the key frame came still went out; the one waiting gave way.
        assertEquals(listOf(0L, 600_000L), peer.frames.map { it.timestampUs })
    }

    @Test
    fun aKeyFrameRequestedThatDoesNotComeIsAskedForAgainAfterASecond() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)
        capture.frame(0, key = true)

        engine.keyFrameRequested(peer)
        capture.frame(16)
        capture.frame(999)
        assertEquals(1, capture.keyFrameRequests)
        capture.frame(1_001)
        runCurrent()

        assertEquals(2, capture.keyFrameRequests)
        // What the desktop drops anyway is not sent.
        assertEquals(listOf(0L), peer.frames.map { it.timestampUs })
    }

    @Test
    fun aKeyFrameRequestFromAnotherDesktopIsIgnored() = runTest {
        val engine = engine()
        val capture = share(engine, FakePeer("a"))

        engine.keyFrameRequested(FakePeer("b"))

        assertEquals(0, capture.keyFrameRequests)
    }

    @Test
    fun timestampsGrowAlsoWhenTheCaptureStartsOver() = runTest {
        val engine = engine()
        val peer = FakePeer("a")
        val capture = share(engine, peer)

        capture.frame(100, key = true)
        runCurrent()
        capture.format()
        capture.frame(0, key = true)
        runCurrent()

        val (first, second) = peer.frames.map { it.timestampUs }
        assertTrue(second > first)
    }

    @Test
    fun noMoreThanSixtyFramesASecondGoOut() = runTest {
        val engine = engine(encodable = setOf(ScreenCodec.SCREEN_CODEC_JPEG))
        val peer = FakePeer("a")
        val capture = share(engine, peer, request(listOf(ScreenCodec.SCREEN_CODEC_JPEG)))

        // JPEG frames, all key frames, at 120 a second for a second.
        for (i in 0 until 120) {
            capture.sink.onFrame(i * 8_333L, true, ByteArray(10))
            runCurrent()
        }

        assertTrue(peer.frames.size in 60..61)
    }
}
