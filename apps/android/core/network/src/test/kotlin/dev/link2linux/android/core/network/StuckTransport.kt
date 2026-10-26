package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.Envelope
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * A connection to a desktop that the test plays by hand: it [feed]s the
 * desktop's frames, and from [stick] on every write blocks, as when the
 * desktop stops reading, until [release] or [close].
 */
class StuckTransport : Transport {
    private val incoming = LinkedBlockingQueue<ByteArray>()
    private val written = ByteArrayOutputStream()
    private val closed = CountDownLatch(1)

    @Volatile
    private var gate: CountDownLatch? = null

    /** Writes that are blocked now. */
    val blockedWrites = AtomicInteger()

    /** Feeds [envelopes] to the phone as the desktop's next frames. */
    fun feed(vararg envelopes: Envelope) {
        val bytes = ByteArrayOutputStream()
        envelopes.forEach { FrameCodec().write(it, bytes) }
        incoming.put(bytes.toByteArray())
    }

    /** From now on, writes block until [release]. */
    fun stick() {
        gate = CountDownLatch(1)
    }

    /** Lets the blocked writes, and every later one, through. */
    fun release() {
        gate?.countDown()
        gate = null
    }

    /** Waits until a write is blocked, so the session's writer is stuck. */
    fun awaitBlockedWrite() {
        val until = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
        while (blockedWrites.get() == 0) {
            check(System.nanoTime() < until) { "no write blocked" }
            Thread.sleep(5)
        }
    }

    /** The envelopes the phone wrote. */
    fun envelopes(): List<Envelope> {
        val bytes = ByteArrayInputStream(synchronized(written) { written.toByteArray() })
        return generateSequence { FrameCodec().read(bytes) }.toList()
    }

    override val input: InputStream = object : InputStream() {
        private var current = ByteArray(0)
        private var position = 0

        override fun read(): Int {
            val one = ByteArray(1)
            return if (read(one, 0, 1) == -1) -1 else one[0].toInt() and 0xff
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            while (position == current.size) {
                if (closed.count == 0L) return -1
                current = incoming.poll(10, TimeUnit.MILLISECONDS) ?: continue
                position = 0
            }
            val n = minOf(len, current.size - position)
            System.arraycopy(current, position, b, off, n)
            position += n
            return n
        }
    }

    override val output: OutputStream = object : OutputStream() {
        override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)

        override fun write(b: ByteArray, off: Int, len: Int) {
            gate?.let {
                blockedWrites.incrementAndGet()
                try {
                    it.await()
                } finally {
                    blockedWrites.decrementAndGet()
                }
            }
            if (closed.count == 0L) throw IOException("Socket closed")
            synchronized(written) { written.write(b, off, len) }
        }
    }

    override fun close() {
        closed.countDown()
        gate?.countDown()
    }
}
