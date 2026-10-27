package dev.link2linux.android.core.ring

import dev.link2linux.protocol.v1.RingEndReason
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RingEngineTest {

    private class FakePlayer : RingPlayer {
        var canRing = true
        var playing = false
        var starts = 0

        /** Each start's way to tell the engine it cannot go on, oldest first. */
        val interruptions = mutableListOf<() -> Unit>()

        override fun start(onInterrupted: () -> Unit): Boolean {
            if (!canRing) return false
            starts++
            playing = true
            interruptions += onInterrupted
            return true
        }

        override fun stop() {
            playing = false
        }

        /** What the platform does when a call starts: tells the ringing that started last. */
        fun interrupt() = interruptions.last().invoke()
    }

    private class FakeVibrator : RingVibrator {
        var vibrating = false

        override fun start() {
            vibrating = true
        }

        override fun stop() {
            vibrating = false
        }
    }

    private class FakeNotifier : RingNotifier {
        var shown: String? = null

        override fun show(desktopName: String) {
            shown = desktopName
        }

        override fun cancel() {
            shown = null
        }
    }

    private class FakePeer(override val desktopId: String) : RingPeer {
        val ended = mutableListOf<RingEndReason>()

        override suspend fun ended(reason: RingEndReason) {
            ended += reason
        }
    }

    private val player = FakePlayer()
    private val vibrator = FakeVibrator()
    private val notifier = FakeNotifier()
    private val enabled = MutableStateFlow(true)

    private fun TestScope.engine() = RingEngine(backgroundScope, player, vibrator, notifier, enabled)

    private suspend fun RingEngine.ring(peer: FakePeer, name: String = peer.desktopId): RingAnswer {
        var answer: RingAnswer? = null
        ring(peer.desktopId, name) { answer = it }
        return checkNotNull(answer)
    }

    private val ringing: Boolean get() = player.playing && vibrator.vibrating && notifier.shown != null

    @Test
    fun aRequestRingsVibratesAndNamesTheDesktop() = runTest {
        val engine = engine()

        assertEquals(RingAnswer.RINGING, engine.ring(FakePeer("a"), "Laptop"))

        assertTrue(ringing)
        assertEquals("Laptop", notifier.shown)
        assertEquals("Laptop", engine.ringingFor.value)
    }

    @Test
    fun itRingsForOneMinuteThenTellsTheDesktop() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.attach(a)
        engine.ring(a)

        advanceTimeBy(RingEngine.RING_MILLIS - 1)
        runCurrent()
        assertTrue(ringing)

        advanceTimeBy(1)
        runCurrent()
        assertFalse(player.playing)
        assertFalse(vibrator.vibrating)
        assertNull(notifier.shown)
        assertNull(engine.ringingFor.value)
        assertEquals(listOf(RingEndReason.RING_END_REASON_TIMED_OUT), a.ended)
    }

    @Test
    fun aRequestWhileItRingsFromAnyDesktopStartsTheMinuteAgain() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        val b = FakePeer("b")
        val bystander = FakePeer("c")
        listOf(a, b, bystander).forEach { engine.attach(it) }
        engine.ring(a, "Laptop")

        advanceTimeBy(30_000)
        assertEquals(RingAnswer.RINGING, engine.ring(b, "Desktop"))
        assertEquals(1, player.starts)
        assertEquals("Desktop", notifier.shown)

        advanceTimeBy(RingEngine.RING_MILLIS - 1)
        runCurrent()
        assertTrue(ringing)

        advanceTimeBy(1)
        runCurrent()
        assertFalse(ringing)
        assertEquals(listOf(RingEndReason.RING_END_REASON_TIMED_OUT), a.ended)
        assertEquals(listOf(RingEndReason.RING_END_REASON_TIMED_OUT), b.ended)
        assertEquals(emptyList<RingEndReason>(), bystander.ended)
    }

    @Test
    fun aDesktopStopsIt() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        val b = FakePeer("b")
        engine.attach(a)
        engine.attach(b)
        engine.ring(a)
        engine.ring(b)

        engine.stopFromDesktop()
        runCurrent()

        assertFalse(ringing)
        assertEquals(listOf(RingEndReason.RING_END_REASON_STOPPED_FROM_DESKTOP), a.ended)
        assertEquals(listOf(RingEndReason.RING_END_REASON_STOPPED_FROM_DESKTOP), b.ended)
        // The minute that was left ends nothing more.
        advanceTimeBy(2 * RingEngine.RING_MILLIS)
        runCurrent()
        assertEquals(1, a.ended.size)
    }

    @Test
    fun aStopWhileItDoesNotRingDoesNothing() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.attach(a)

        engine.stopFromDesktop()
        runCurrent()

        assertEquals(emptyList<RingEndReason>(), a.ended)
    }

    @Test
    fun theUserStopsItOnThePhone() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.attach(a)
        engine.ring(a)

        engine.stopOnPhone()
        runCurrent()

        assertFalse(ringing)
        assertEquals(listOf(RingEndReason.RING_END_REASON_STOPPED_ON_PHONE), a.ended)
    }

    @Test
    fun aCallInterruptsIt() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.attach(a)
        engine.ring(a)

        player.interrupt()
        runCurrent()

        assertFalse(ringing)
        assertEquals(listOf(RingEndReason.RING_END_REASON_INTERRUPTED), a.ended)
    }

    @Test
    fun aLateInterruptionOfAnEarlierRingingLeavesTheNextOneRinging() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.attach(a)
        engine.ring(a)
        val late = player.interruptions.single()
        engine.stopFromDesktop()
        runCurrent()
        engine.ring(a)

        late()
        runCurrent()

        assertTrue(ringing)
        assertEquals(listOf(RingEndReason.RING_END_REASON_STOPPED_FROM_DESKTOP), a.ended)
    }

    @Test
    fun duringACallTheRequestIsRefused() = runTest {
        val engine = engine()
        player.canRing = false

        assertEquals(RingAnswer.UNAVAILABLE, engine.ring(FakePeer("a")))

        assertFalse(vibrator.vibrating)
        assertNull(notifier.shown)
        assertNull(engine.ringingFor.value)
    }

    @Test
    fun whileTheFeatureIsOffTheRequestIsRefused() = runTest {
        enabled.value = false
        val engine = engine()

        assertEquals(RingAnswer.DISABLED, engine.ring(FakePeer("a")))

        assertFalse(player.playing)
    }

    @Test
    fun turningTheFeatureOffInterruptsIt() = runTest {
        val engine = engine()
        val a = FakePeer("a")
        engine.attach(a)
        runCurrent()
        engine.ring(a)

        enabled.value = false
        runCurrent()

        assertFalse(ringing)
        assertEquals(listOf(RingEndReason.RING_END_REASON_INTERRUPTED), a.ended)
    }

    @Test
    fun ringingOutlivesTheSessionAndAReconnectedDesktopIsTold() = runTest {
        val engine = engine()
        val first = FakePeer("a")
        engine.attach(first)
        engine.ring(first)

        engine.detach(first)
        advanceTimeBy(30_000)
        runCurrent()
        assertTrue(ringing)

        val again = FakePeer("a")
        engine.attach(again)
        advanceTimeBy(30_000)
        runCurrent()

        assertFalse(ringing)
        assertEquals(emptyList<RingEndReason>(), first.ended)
        assertEquals(listOf(RingEndReason.RING_END_REASON_TIMED_OUT), again.ended)
    }

    @Test
    fun theAnswerComesBeforeTheEndOfTheRingingItStarted() = runTest {
        val engine = engine()
        val events = mutableListOf<String>()
        val a = object : RingPeer {
            override val desktopId = "a"

            override suspend fun ended(reason: RingEndReason) {
                events += "ended"
            }
        }
        engine.attach(a)
        // An interruption that arrives while the request is being answered.
        engine.ring("a", "Laptop") { answer ->
            player.interrupt()
            events += answer.name
        }
        runCurrent()

        assertEquals(listOf("RINGING", "ended"), events)
    }
}
