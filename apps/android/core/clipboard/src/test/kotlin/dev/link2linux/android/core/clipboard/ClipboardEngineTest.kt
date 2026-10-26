package dev.link2linux.android.core.clipboard

import dev.link2linux.protocol.v1.ClipboardPush
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.clipboardPush
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException

@OptIn(ExperimentalCoroutinesApi::class)
class ClipboardEngineTest {

    /** A desktop whose answers the test gives, unless it has an [autoAnswer]. */
    private class FakePeer(override val desktopId: String, private val autoAnswer: PushAnswer? = null) : ClipboardPeer {
        class Call(val push: ClipboardPush) {
            val answer = CompletableDeferred<PushAnswer>()
        }

        override val name = "Desk $desktopId"
        val calls = mutableListOf<Call>()

        override suspend fun push(push: ClipboardPush): PushAnswer {
            val call = Call(push)
            calls += call
            return autoAnswer ?: call.answer.await()
        }

        fun texts() = calls.map { it.push.text }
    }

    private class FakeWriter : ClipboardWriter {
        val written = mutableListOf<ClipText>()
        var refusal: Exception? = null

        override suspend fun write(clip: ClipText) {
            refusal?.let { throw it }
            written += clip
        }
    }

    private val enabled = MutableStateFlow(true)
    private val writer = FakeWriter()
    private val errors = mutableListOf<Throwable>()

    private fun TestScope.engine(
        scope: CoroutineScope = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler)),
    ) = ClipboardEngine(scope = scope, writer = writer, enabled = enabled, onError = { errors += it })

    /** Attaches [peer] with the feature ready there. */
    private suspend fun ClipboardEngine.ready(peer: FakePeer) {
        attach(peer)
        onDesktopStatus(peer, FeatureAvailability.FEATURE_AVAILABILITY_READY)
    }

    private fun push(text: String, sensitive: Boolean = false) = clipboardPush {
        this.text = text
        this.sensitive = sensitive
    }

    private fun pushed(vararg outcomes: Pair<String, PushOutcome>) =
        ShareResult.Pushed(outcomes.map { (id, outcome) -> DesktopOutcome("Desk $id", outcome) })

    private fun ClipboardReply.code() = (this as ClipboardReply.Error).code

    // 256 KiB of UTF-8 in half as many characters.
    private val longest = "é".repeat(MAX_TEXT_BYTES / 2)

    @Test
    fun aPushGoesOnTheClipboardWithItsSensitiveMark() = runTest {
        val engine = engine()

        assertEquals(ClipboardReply.Ack, engine.receive("a", push("secret", sensitive = true)))
        assertEquals(ClipboardReply.Ack, engine.receive("a", push("plain")))

        assertEquals(listOf(ClipText("secret", sensitive = true), ClipText("plain")), writer.written)
    }

    @Test
    fun aPushWhileTheFeatureIsOffIsRefused() = runTest {
        enabled.value = false

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, engine().receive("a", push("hi")).code())
        assertTrue(writer.written.isEmpty())
    }

    @Test
    fun emptyTextAndTextOverTheLimitAreInvalid() = runTest {
        val engine = engine()

        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.receive("a", push("")).code())
        // The limit counts UTF-8 bytes, not characters.
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, engine.receive("a", push(longest + "a")).code())
        assertTrue(writer.written.isEmpty())

        assertEquals(ClipboardReply.Ack, engine.receive("a", push(longest)))
    }

    @Test
    fun aClipboardThatRefusesTheTextGivesUnavailable() = runTest {
        writer.refusal = SecurityException("not now")

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, engine().receive("a", push("hi")).code())
        assertEquals(listOf(writer.refusal), errors)
    }

    @Test
    fun aShareGoesToEveryReadyDesktop() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        val b = FakePeer("b")
        engine.ready(a)
        engine.ready(b)

        val result = async { engine.share(ClipText("hello", sensitive = true)) }
        runCurrent()
        assertEquals(listOf("hello"), a.texts())
        assertEquals(listOf("hello"), b.texts())
        assertTrue(a.calls.single().push.sensitive)
        a.calls.single().answer.complete(PushAnswer.Ack)
        b.calls.single().answer.complete(PushAnswer.Ack)

        assertEquals(pushed("a" to PushOutcome.SENT, "b" to PushOutcome.SENT), result.await())
    }

    @Test
    fun aShareIsCheckedBeforeAnythingIsSent() = runTest {
        val engine = engine()
        assertEquals(ShareResult.NoDesktop, engine.share(ClipText("hi")))

        val a = FakePeer("a", autoAnswer = PushAnswer.Ack)
        engine.ready(a)
        assertEquals(ShareResult.Empty, engine.share(ClipText("")))
        assertEquals(ShareResult.TooLarge, engine.share(ClipText(longest + "a")))
        enabled.value = false
        assertEquals(ShareResult.Disabled, engine.share(ClipText("hi")))
        assertTrue(a.calls.isEmpty())

        enabled.value = true
        assertEquals(pushed("a" to PushOutcome.SENT), engine.share(ClipText(longest)))
    }

    @Test
    fun aDesktopThatIsNotReadyGetsNothing() = runTest {
        val engine = engine()
        val unknown = FakePeer("u")
        val off = FakePeer("o")
        val unavailable = FakePeer("n")
        engine.attach(unknown)
        engine.attach(off)
        engine.onDesktopStatus(off, FeatureAvailability.FEATURE_AVAILABILITY_DISABLED)
        engine.attach(unavailable)
        engine.onDesktopStatus(unavailable, FeatureAvailability.FEATURE_AVAILABILITY_UNAVAILABLE)

        val result = engine.share(ClipText("hi"))

        assertEquals(pushed("u" to PushOutcome.FAILED, "o" to PushOutcome.TURNED_OFF, "n" to PushOutcome.FAILED), result)
        assertTrue(unknown.calls.isEmpty() && off.calls.isEmpty() && unavailable.calls.isEmpty())
    }

    @Test
    fun theDesktopsAnswerDecidesTheOutcome() = runTest {
        val cases = listOf(
            PushAnswer.Error(ErrorCode.ERROR_CODE_DISABLED) to PushOutcome.TURNED_OFF,
            PushAnswer.Error(ErrorCode.ERROR_CODE_UNAVAILABLE) to PushOutcome.FAILED,
            PushAnswer.Error(ErrorCode.ERROR_CODE_INVALID_ARGUMENT) to PushOutcome.FAILED,
            PushAnswer.NoAnswer to PushOutcome.FAILED,
        )
        for ((answer, expected) in cases) {
            val engine = engine()
            engine.ready(FakePeer("a", autoAnswer = answer))
            assertEquals("$answer", pushed("a" to expected), engine.share(ClipText("hi")))
        }
    }

    @Test
    fun aConnectionThatFailsDuringAPushFailsIt() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.ready(a)

        val result = async { engine.share(ClipText("hi")) }
        runCurrent()
        a.calls.single().answer.completeExceptionally(IOException("Broken pipe"))

        assertEquals(pushed("a" to PushOutcome.FAILED), result.await())
    }

    @Test
    fun onePushWaitsPerDesktopAndOnlyTheNewestQueuedShareIsSent() = runTest {
        val engine = engine()
        val slow = FakePeer("a")
        val quick = FakePeer("b", autoAnswer = PushAnswer.Ack)
        engine.ready(slow)
        engine.ready(quick)

        val first = async { engine.share(ClipText("1")) }
        runCurrent()
        val second = async { engine.share(ClipText("2")) }
        runCurrent()
        val third = async { engine.share(ClipText("3")) }
        runCurrent()

        // The slow desktop holds "1" up, but only for itself.
        assertEquals(listOf("1"), slow.texts())
        assertEquals(listOf("1", "2", "3"), quick.texts())
        assertEquals(pushed("a" to PushOutcome.REPLACED, "b" to PushOutcome.SENT), second.await())

        slow.calls[0].answer.complete(PushAnswer.Ack)
        runCurrent()
        assertEquals(listOf("1", "3"), slow.texts())
        slow.calls[1].answer.complete(PushAnswer.Ack)

        assertEquals(pushed("a" to PushOutcome.SENT, "b" to PushOutcome.SENT), first.await())
        assertEquals(pushed("a" to PushOutcome.SENT, "b" to PushOutcome.SENT), third.await())

        // Nothing waits any more: the next share goes out at once.
        val fourth = async { engine.share(ClipText("4")) }
        runCurrent()
        assertEquals(listOf("1", "3", "4"), slow.texts())
        slow.calls[2].answer.complete(PushAnswer.Ack)
        assertEquals(pushed("a" to PushOutcome.SENT, "b" to PushOutcome.SENT), fourth.await())
    }

    /** Shares "1" (left waiting for its answer) and "2" (queued behind it) to [peer]. */
    private suspend fun TestScope.inFlightAndQueued(engine: ClipboardEngine, peer: FakePeer): Pair<Deferred<ShareResult>, Deferred<ShareResult>> {
        val first = async { engine.share(ClipText("1")) }
        runCurrent()
        val second = async { engine.share(ClipText("2")) }
        runCurrent()
        assertEquals(listOf("1"), peer.texts())
        return first to second
    }

    @Test
    fun aQueuedShareIsNotSentOnceThePhoneTurnsTheFeatureOff() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.ready(a)
        val (first, second) = inFlightAndQueued(engine, a)

        enabled.value = false
        a.calls[0].answer.complete(PushAnswer.Ack)

        assertEquals(pushed("a" to PushOutcome.SENT), first.await())
        assertEquals(pushed("a" to PushOutcome.FAILED), second.await())
        assertEquals(listOf("1"), a.texts())
    }

    @Test
    fun aQueuedShareIsNotSentOnceTheDesktopIsNoLongerReady() = runTest {
        val cases = listOf(
            FeatureAvailability.FEATURE_AVAILABILITY_DISABLED to PushOutcome.TURNED_OFF,
            FeatureAvailability.FEATURE_AVAILABILITY_UNAVAILABLE to PushOutcome.FAILED,
        )
        for ((availability, expected) in cases) {
            val engine = engine()
            val a = FakePeer("a")
            engine.ready(a)
            val (first, second) = inFlightAndQueued(engine, a)

            engine.onDesktopStatus(a, availability)
            a.calls[0].answer.complete(PushAnswer.Ack)

            assertEquals(pushed("a" to PushOutcome.SENT), first.await())
            assertEquals("$availability", pushed("a" to expected), second.await())
            assertEquals(listOf("1"), a.texts())
        }
    }

    @Test
    fun aDisabledAnswerHoldsSharesBackUntilTheDesktopIsReadyAgain() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.ready(a)
        val (first, second) = inFlightAndQueued(engine, a)

        a.calls[0].answer.complete(PushAnswer.Error(ErrorCode.ERROR_CODE_DISABLED))

        assertEquals(pushed("a" to PushOutcome.TURNED_OFF), first.await())
        assertEquals(pushed("a" to PushOutcome.TURNED_OFF), second.await())
        assertEquals(false, engine.connected.value.single().isReady)
        assertEquals(pushed("a" to PushOutcome.TURNED_OFF), engine.share(ClipText("3")))

        engine.onDesktopStatus(a, FeatureAvailability.FEATURE_AVAILABILITY_READY)
        val fourth = async { engine.share(ClipText("4")) }
        runCurrent()
        a.calls[1].answer.complete(PushAnswer.Ack)
        assertEquals(pushed("a" to PushOutcome.SENT), fourth.await())
        assertEquals(listOf("1", "4"), a.texts())
    }

    @Test
    fun aStatusThatCameDuringThePushOutranksItsDisabledAnswer() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.ready(a)
        val (first, second) = inFlightAndQueued(engine, a)

        // The desktop turned the feature off, answered, then turned it on again.
        engine.onDesktopStatus(a, FeatureAvailability.FEATURE_AVAILABILITY_READY)
        a.calls[0].answer.complete(PushAnswer.Error(ErrorCode.ERROR_CODE_DISABLED))
        runCurrent()

        assertEquals(pushed("a" to PushOutcome.TURNED_OFF), first.await())
        assertTrue(engine.connected.value.single().isReady)
        assertEquals(listOf("1", "2"), a.texts())
        a.calls[1].answer.complete(PushAnswer.Ack)
        assertEquals(pushed("a" to PushOutcome.SENT), second.await())
    }

    @Test
    fun theRefusalBeforeSharingSaysWhatAnyShareWouldComeTo() = runTest {
        val engine = engine()
        assertEquals(ShareResult.NoDesktop, engine.refusalBeforeSharing())

        val off = FakePeer("o")
        val unknown = FakePeer("u")
        engine.attach(off)
        engine.onDesktopStatus(off, FeatureAvailability.FEATURE_AVAILABILITY_DISABLED)
        engine.attach(unknown)
        assertEquals(pushed("o" to PushOutcome.TURNED_OFF, "u" to PushOutcome.FAILED), engine.refusalBeforeSharing())

        engine.onDesktopStatus(unknown, FeatureAvailability.FEATURE_AVAILABILITY_READY)
        assertEquals(null, engine.refusalBeforeSharing())

        enabled.value = false
        assertEquals(ShareResult.Disabled, engine.refusalBeforeSharing())
        assertTrue(off.calls.isEmpty() && unknown.calls.isEmpty())
    }

    @Test
    fun aDesktopThatLeavesFailsWhatWaitsForIt() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.ready(a)

        val first = async { engine.share(ClipText("1")) }
        runCurrent()
        val second = async { engine.share(ClipText("2")) }
        runCurrent()
        engine.detach(a)

        assertEquals(pushed("a" to PushOutcome.FAILED), second.await())
        a.calls.single().answer.completeExceptionally(IOException("Socket closed"))
        assertEquals(pushed("a" to PushOutcome.FAILED), first.await())
        assertEquals(ShareResult.NoDesktop, engine.share(ClipText("3")))
        assertEquals(listOf("1"), a.texts())
    }

    @Test
    fun aNewSessionWithTheSameDesktopReplacesTheOldOne() = runTest {
        val engine = engine()
        val old = FakePeer("a")
        val replacement = FakePeer("a", autoAnswer = PushAnswer.Ack)
        engine.ready(old)
        engine.ready(replacement)

        assertEquals(1, engine.connected.value.size)
        assertEquals(pushed("a" to PushOutcome.SENT), engine.share(ClipText("hi")))
        assertTrue(old.calls.isEmpty())

        // The old session ending later changes nothing.
        engine.detach(old)
        assertEquals(1, engine.connected.value.size)
    }

    @Test
    fun anExplicitShareGoesToEveryDesktopWhateverItSentBefore() = runTest {
        val engine = engine()
        val a = FakePeer("a", autoAnswer = PushAnswer.Ack)
        val b = FakePeer("b", autoAnswer = PushAnswer.Ack)
        engine.ready(a)
        engine.ready(b)
        engine.receive("a", push("from a"))

        repeat(2) {
            assertEquals(pushed("a" to PushOutcome.SENT, "b" to PushOutcome.SENT), engine.share(ClipText("from a")))
        }
        assertEquals(listOf("from a", "from a"), a.texts())
    }

    @Test
    fun anAutomaticShareNeitherEchoesNorRelaysNorRepeats() = runTest {
        val engine = engine()
        val a = FakePeer("a", autoAnswer = PushAnswer.Ack)
        val b = FakePeer("b", autoAnswer = PushAnswer.Ack)
        val notReady = FakePeer("c")
        engine.ready(a)
        engine.ready(b)
        engine.attach(notReady)
        engine.receive("a", push("from a"))

        // Not back to a, nor on to b.
        assertEquals(ShareResult.Pushed(emptyList()), engine.share(ClipText("from a"), automatic = true))
        // New text goes to the ready desktops, once.
        assertEquals(pushed("a" to PushOutcome.SENT, "b" to PushOutcome.SENT), engine.share(ClipText("new"), automatic = true))
        assertEquals(ShareResult.Pushed(emptyList()), engine.share(ClipText("new"), automatic = true))

        assertEquals(listOf("new"), a.texts())
        assertEquals(listOf("new"), b.texts())
        assertTrue(notReady.calls.isEmpty())
    }

    @Test
    fun anAutomaticShareThatFailedIsNotTakenAsPushed() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.ready(a)

        val failed = async { engine.share(ClipText("t"), automatic = true) }
        runCurrent()
        a.calls[0].answer.complete(PushAnswer.Error(ErrorCode.ERROR_CODE_UNAVAILABLE))
        assertEquals(pushed("a" to PushOutcome.FAILED), failed.await())

        val again = async { engine.share(ClipText("t"), automatic = true) }
        runCurrent()
        a.calls[1].answer.complete(PushAnswer.Ack)
        assertEquals(pushed("a" to PushOutcome.SENT), again.await())
    }

    @Test
    fun connectedFollowsTheDesktopsAndTheirStatus() = runTest {
        val engine = engine()
        val a = FakePeer("a")

        engine.attach(a)
        assertEquals(listOf(ConnectedDesktop("Desk a", FeatureAvailability.FEATURE_AVAILABILITY_UNSPECIFIED)), engine.connected.value)
        engine.onDesktopStatus(a, FeatureAvailability.FEATURE_AVAILABILITY_READY)
        assertTrue(engine.connected.value.single().isReady)
        engine.onDesktopStatus(a, FeatureAvailability.FEATURE_AVAILABILITY_DISABLED)
        assertEquals(false, engine.connected.value.single().isReady)
        engine.detach(a)
        assertEquals(emptyList<ConnectedDesktop>(), engine.connected.value)
    }

    @Test
    fun aShareStillHearsBackWhenTheEngineStops() = runTest {
        val scope = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(testScheduler))
        val engine = engine(scope)
        val a = FakePeer("a")
        engine.ready(a)

        val first = async { engine.share(ClipText("1")) }
        runCurrent()
        val second = async { engine.share(ClipText("2")) }
        runCurrent()
        scope.cancel()

        assertEquals(pushed("a" to PushOutcome.FAILED), first.await())
        assertEquals(pushed("a" to PushOutcome.FAILED), second.await())
    }
}
