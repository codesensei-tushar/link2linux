package dev.link2linux.android.feature.files

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ShareFlowTest {
    private val prepared = mutableListOf<List<String>>()
    private val reading = CompletableDeferred<String>()
    private val sent = mutableListOf<Pair<String, String>>()
    private val discarded = mutableListOf<String>()

    private fun TestScope.flow() = ShareFlow<String, String, String>(
        scope = backgroundScope,
        read = { documents ->
            prepared += documents
            reading
        },
        send = { desktop, files -> sent += desktop to files },
        discard = { discarded += it },
    )

    @Test
    fun `nothing is sent before the user confirms`() = runTest {
        val share = flow()
        share.prepare(listOf("a", "b"))
        assertEquals(ShareState.Preparing, share.state.value)
        reading.complete("a+b")
        testScheduler.runCurrent()
        assertEquals(ShareState.Ready("a+b"), share.state.value)
        assertTrue(sent.isEmpty())
        share.confirm("laptop")
        assertEquals(listOf("laptop" to "a+b"), sent)
        assertEquals(ShareState.Done("laptop"), share.state.value)
        // Confirmed twice, or closed afterwards, it is still sent once and kept.
        share.confirm("laptop")
        share.close()
        assertEquals(1, sent.size)
        assertTrue(discarded.isEmpty())
    }

    @Test
    fun `the documents are read once, however often the activity asks`() = runTest {
        val share = flow()
        share.prepare(listOf("a"))
        share.prepare(listOf("a"))
        assertEquals(1, prepared.size)
    }

    @Test
    fun `a share cancelled while it is read stops the read`() = runTest {
        val share = flow()
        share.prepare(listOf("a"))
        share.cancel()
        assertEquals(ShareState.Done(null), share.state.value)
        // The read stops, and lets go of what it took itself: nothing is left to discard.
        assertTrue(reading.isCancelled)
        testScheduler.runCurrent()
        assertTrue(discarded.isEmpty())
        assertEquals(ShareState.Done(null), share.state.value)
        share.confirm("laptop")
        assertTrue(sent.isEmpty())
    }

    @Test
    fun `a share closed while it is read stops the read`() = runTest {
        val share = flow()
        share.prepare(listOf("a"))
        share.close()
        assertTrue(reading.isCancelled)
    }

    @Test
    fun `a share cancelled once read is let go`() = runTest {
        val share = flow()
        share.prepare(listOf("a"))
        reading.complete("a")
        testScheduler.runCurrent()
        share.cancel()
        assertEquals(listOf("a"), discarded)
        assertTrue(sent.isEmpty())
    }

    @Test
    fun `a share left without an answer is let go`() = runTest {
        val share = flow()
        share.prepare(listOf("a"))
        reading.complete("a")
        testScheduler.runCurrent()
        share.close()
        assertEquals(listOf("a"), discarded)
        assertTrue(sent.isEmpty())
    }
}
