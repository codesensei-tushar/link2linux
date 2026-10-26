package dev.link2linux.android.feature.notifications

import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DeadlineReaderTest {
    private val bytes = ByteArray(100) { it.toByte() }

    /** A stream that never sends a byte, until it is closed. */
    private class SilentStream : InputStream() {
        val closed = CountDownLatch(1)

        override fun read(): Int {
            closed.await(10, TimeUnit.SECONDS)
            throw IOException("closed")
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int = read()

        override fun close() = closed.countDown()
    }

    private fun millisOf(block: () -> Unit): Long {
        val start = System.nanoTime()
        block()
        return (System.nanoTime() - start) / 1_000_000
    }

    @Test(timeout = 10_000)
    fun `what a source serves within the limits is read whole`() {
        val reader = DeadlineReader(maxBytes = 100, deadlineMillis = 2_000, maxReads = 4, maxPerSource = 2)
        assertArrayEquals(bytes, reader.read("app") { ByteArrayInputStream(bytes) })
        assertNull(reader.read("app") { ByteArrayInputStream(ByteArray(101)) })
        assertNull(reader.read("app") { null })
        assertNull(reader.read("app") { throw SecurityException("not exported") })
    }

    @Test(timeout = 10_000)
    fun `an open that never returns gives nothing at the deadline`() {
        val reader = DeadlineReader(maxBytes = 1024, deadlineMillis = 200, maxReads = 4, maxPerSource = 2)
        val answer = CountDownLatch(1)
        val took = millisOf { assertNull(reader.read("app") { answer.await(); ByteArrayInputStream(bytes) }) }
        assertTrue("took $took ms", took < 2_000)
        answer.countDown()
    }

    @Test(timeout = 10_000)
    fun `a stream that never ends is closed at the deadline`() {
        val reader = DeadlineReader(maxBytes = 1024, deadlineMillis = 200, maxReads = 4, maxPerSource = 2)
        val stream = SilentStream()
        val took = millisOf { assertNull(reader.read("app") { stream }) }
        assertTrue("took $took ms", took < 2_000)
        assertTrue(stream.closed.await(5, TimeUnit.SECONDS))
    }

    @Test(timeout = 20_000)
    fun `opens that never return hold a bounded number of threads, and free them when they return`() {
        val reader = DeadlineReader(maxBytes = 1024, deadlineMillis = 100, maxReads = 3, maxPerSource = 2)
        val answer = CountDownLatch(1)
        val opened = AtomicInteger()
        val late = mutableListOf<SilentStream>()
        val stuck: () -> InputStream = {
            opened.incrementAndGet()
            answer.await()
            SilentStream().also { synchronized(late) { late += it } }
        }

        assertNull(reader.read("bad") { stuck() })
        assertNull(reader.read("bad") { stuck() })
        // A third read from the same source is refused at once, without calling into it.
        val took = millisOf { assertNull(reader.read("bad") { stuck() }) }
        assertTrue("took $took ms", took < 1_000)
        assertEquals(2, opened.get())
        // Another source still gets its images, until stuck reads fill every place.
        assertArrayEquals(bytes, reader.read("good") { ByteArrayInputStream(bytes) })
        assertNull(reader.read("other") { stuck() })
        assertNull(reader.read("good") { ByteArrayInputStream(bytes) })
        assertEquals(3, opened.get())

        // When the opens return at last, their streams are closed unread and the places are free again.
        answer.countDown()
        var again: ByteArray? = null
        val until = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
        while (again == null && System.nanoTime() < until) {
            again = reader.read("bad") { ByteArrayInputStream(bytes) }
            if (again == null) Thread.sleep(20)
        }
        assertArrayEquals(bytes, again)
        while (synchronized(late) { late.size } < 3 && System.nanoTime() < until) Thread.sleep(20)
        synchronized(late) {
            assertEquals(3, late.size)
            late.forEach { assertTrue(it.closed.await(5, TimeUnit.SECONDS)) }
        }
    }
}
