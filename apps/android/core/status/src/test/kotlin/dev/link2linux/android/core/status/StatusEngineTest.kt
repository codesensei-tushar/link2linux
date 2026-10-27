package dev.link2linux.android.core.status

import dev.link2linux.protocol.v1.Battery
import dev.link2linux.protocol.v1.Cellular
import dev.link2linux.protocol.v1.PhoneStatus
import dev.link2linux.protocol.v1.Wifi
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class StatusEngineTest {

    /** The platform as a value the test sets; counts the listeners it runs. */
    private class FakeSource(first: PhoneStatus) : StatusSource {
        val status = MutableStateFlow(first)
        var listeners = 0

        override fun statuses(): Flow<PhoneStatus> = flow {
            listeners++
            try {
                emitAll(status)
            } finally {
                listeners--
            }
        }
    }

    private fun battery(percent: Int): PhoneStatus =
        PhoneStatus.newBuilder().setBattery(Battery.newBuilder().setPercent(percent)).build()

    private val enabled = MutableStateFlow(true)
    private val source = FakeSource(battery(50))

    /** Follows the engine's status from now, as a desktop that got the reply; gives what it sends, with the time. */
    private suspend fun TestScope.follow(engine: StatusEngine): MutableList<Pair<Long, Int>> {
        val sent = mutableListOf<Pair<Long, Int>>()
        val reply = engine.current()
        backgroundScope.launch { engine.follow(reply) { sent += testScheduler.currentTime to it.battery.percent } }
        runCurrent()
        return sent
    }

    @Test
    fun theCurrentStatusIsTheSources() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)

        assertEquals(battery(50), engine.current())
    }

    @Test
    fun aChangeWaitsFiveSecondsFromTheReplyAndOnlyTheNewestGoesOut() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)
        val sent = follow(engine)

        advanceTimeBy(1_000)
        source.status.value = battery(49)
        advanceTimeBy(1_000)
        source.status.value = battery(48)
        advanceTimeBy(2_999)
        runCurrent()
        assertEquals(emptyList<Pair<Long, Int>>(), sent)

        advanceTimeBy(1)
        runCurrent()
        assertEquals(listOf(5_000L to 48), sent)
    }

    @Test
    fun afterAnUpdateTheNextOneWaitsAnotherFiveSeconds() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)
        val sent = follow(engine)
        source.status.value = battery(49)
        advanceTimeBy(5_000)
        runCurrent()

        source.status.value = battery(48)
        advanceTimeBy(4_999)
        runCurrent()
        assertEquals(listOf(5_000L to 49), sent)

        advanceTimeBy(1)
        runCurrent()
        assertEquals(listOf(5_000L to 49, 10_000L to 48), sent)
    }

    @Test
    fun aChangeAfterAQuietSpellGoesOutAtOnce() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)
        val sent = follow(engine)

        advanceTimeBy(60_000)
        source.status.value = battery(49)
        runCurrent()

        assertEquals(listOf(60_000L to 49), sent)
    }

    @Test
    fun nothingGoesOutWhenTheStatusChangedBack() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)
        val sent = follow(engine)

        source.status.value = battery(49)
        advanceTimeBy(1_000)
        source.status.value = battery(50)
        advanceTimeBy(30_000)
        runCurrent()

        assertEquals(emptyList<Pair<Long, Int>>(), sent)
    }

    @Test
    fun followingStopsWhenTheUserTurnsTheFeatureOff() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)
        val sent = mutableListOf<Int>()
        val following = backgroundScope.launch { engine.follow(engine.current()) { sent += it.battery.percent } }
        runCurrent()

        enabled.value = false
        runCurrent()
        source.status.value = battery(49)
        advanceTimeBy(30_000)
        runCurrent()

        assertTrue(following.isCompleted)
        assertEquals(emptyList<Int>(), sent)
    }

    @Test
    fun thePlatformIsWatchedOnlyWhileADesktopAsksOrFollows() = runTest {
        val engine = StatusEngine(backgroundScope, source, enabled)
        runCurrent()
        assertEquals(0, source.listeners)

        engine.current()
        runCurrent()
        assertEquals(1, source.listeners)
        advanceTimeBy(StatusEngine.KEEP_LISTENING_MILLIS + 1)
        runCurrent()
        assertEquals(0, source.listeners)

        val following = backgroundScope.launch { engine.follow(engine.current()) {} }
        advanceTimeBy(60_000)
        runCurrent()
        assertEquals(1, source.listeners)

        following.cancel()
        advanceTimeBy(StatusEngine.KEEP_LISTENING_MILLIS + 1)
        runCurrent()
        assertEquals(0, source.listeners)
    }

    @Test
    fun aStatusWithValuesOutOfRangeIsCorrected() = runTest {
        val name = "x".repeat(63) + "\u00e9" // 65 bytes: the last character does not fit
        source.status.value = PhoneStatus.newBuilder()
            .setBattery(Battery.newBuilder().setPercent(101))
            .setCellular(Cellular.newBuilder().setLevel(-1).setOperatorName(name))
            .setWifi(Wifi.newBuilder().setLevel(7))
            .build()
        val engine = StatusEngine(backgroundScope, source, enabled)

        val status = engine.current()

        assertEquals(100, status.battery.percent)
        assertEquals(0, status.cellular.level)
        assertEquals("x".repeat(63), status.cellular.operatorName)
        assertEquals(4, status.wifi.level)
    }

    @Test
    fun truncationKeepsWholeCharacters() {
        assertEquals("abc", truncateUtf8("abc", 64))
        assertEquals("", truncateUtf8("\u00e9", 1))
        assertEquals("\u00e9", truncateUtf8("\u00e9\u00e9", 3))
        // A character outside the BMP is two chars and four bytes; it is kept whole or left out.
        val face = String(Character.toChars(0x1F600))
        assertEquals("ab", truncateUtf8("ab$face", 5))
        assertEquals("ab$face", truncateUtf8("ab$face", 6))
        assertEquals("\u4e2d".repeat(21), truncateUtf8("\u4e2d".repeat(30), MAX_OPERATOR_NAME_BYTES))
    }
}
