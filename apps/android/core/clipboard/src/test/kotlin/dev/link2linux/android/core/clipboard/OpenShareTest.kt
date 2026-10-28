package dev.link2linux.android.core.clipboard

import dev.link2linux.protocol.v1.ClipboardPush
import dev.link2linux.protocol.v1.FeatureAvailability
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenShareTest {

    private class Peer : ClipboardPeer {
        override val desktopId = "a"
        override val name = "Desk"

        override suspend fun push(push: ClipboardPush) = PushAnswer.Ack
    }

    private val featureOn = MutableStateFlow(true)
    private val onOpen = MutableStateFlow(true)
    private val stored = mutableListOf<Long>()
    private val peer = Peer()

    private fun TestScope.engine() = ClipboardEngine(scope = backgroundScope, writer = {}, enabled = featureOn)

    private suspend fun ClipboardEngine.ready() {
        attach(peer)
        onDesktopStatus(peer, FeatureAvailability.FEATURE_AVAILABILITY_READY)
    }

    private fun openShare(engine: ClipboardEngine, handled: Long = 0) = OpenShare(engine, onOpen, handled) { stored += it }

    @Test
    fun eachNewCopyIsReadOnceEvenAfterARestart() = runTest {
        val engine = engine().apply { ready() }
        val gate = openShare(engine)

        assertTrue(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        assertFalse(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        assertEquals(listOf(100L), stored)

        // The app restarted with the stored time.
        val restarted = openShare(engine, handled = stored.last())
        assertFalse(restarted.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        assertTrue(restarted.shouldRead(200, isText = true, sensitive = false, fromComputer = false))
        // A clock set back gives an older time, which is still a new copy.
        assertTrue(restarted.shouldRead(50, isText = true, sensitive = false, fromComputer = false))
        assertEquals(listOf(100L, 200L, 50L), stored)
    }

    @Test
    fun secretsOtherKindsAndTextFromAComputerAreLeftOutAndNotLookedAtAgain() = runTest {
        val gate = openShare(engine().apply { ready() })

        assertFalse(gate.shouldRead(100, isText = true, sensitive = true, fromComputer = false))
        assertFalse(gate.shouldRead(200, isText = false, sensitive = false, fromComputer = false))
        assertFalse(gate.shouldRead(300, isText = true, sensitive = false, fromComputer = true))
        assertEquals(listOf(100L, 200L, 300L), stored)
    }

    @Test
    fun aCopyWaitsWhileNoComputerTakesIt() = runTest {
        val engine = engine()
        val gate = openShare(engine)

        assertFalse(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        engine.attach(peer)
        assertFalse(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        assertEquals(emptyList<Long>(), stored)

        engine.onDesktopStatus(peer, FeatureAvailability.FEATURE_AVAILABILITY_READY)
        assertTrue(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
    }

    @Test
    fun nothingIsReadWhileTheUserHasItOff() = runTest {
        val gate = openShare(engine().apply { ready() })

        onOpen.value = false
        assertFalse(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        onOpen.value = true
        featureOn.value = false
        assertFalse(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        assertEquals(emptyList<Long>(), stored)

        featureOn.value = true
        assertTrue(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
    }

    @Test
    fun aCopyTheUserSentIsNotSentAgainOnOpen() = runTest {
        val gate = openShare(engine().apply { ready() })

        gate.handled(100)
        gate.handled(100)
        assertFalse(gate.shouldRead(100, isText = true, sensitive = false, fromComputer = false))
        assertEquals(listOf(100L), stored)
    }
}
