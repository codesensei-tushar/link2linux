package dev.link2linux.android.feature.notifications

import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.FutureTask
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * Reads what another app serves within [maxBytes] and [deadlineMillis],
 * opening included. Opening a content URI is a call into the app that serves
 * it, which may never return, and nothing can interrupt it; so each read runs
 * on a thread of its own, and at the deadline the caller stops waiting and
 * closes the stream, which ends a read that waits for bytes. An open that
 * never returns keeps its thread, so at most [maxReads] reads run at once,
 * and at most [maxPerSource] for one source; beyond that, a read gets nothing.
 */
internal class DeadlineReader(
    private val maxBytes: Int,
    private val deadlineMillis: Long,
    private val maxReads: Int,
    private val maxPerSource: Int,
) {
    private val threads = Executors.newCachedThreadPool { task -> Thread(task, "l2l-image-read").apply { isDaemon = true } }
    private val running = HashMap<String, Int>() // guarded by itself
    private var total = 0 // guarded by running

    /** What [open] gives, for [source]; null if it gives nothing, more than [maxBytes], or takes too long. */
    fun read(source: String, open: () -> InputStream?): ByteArray? {
        if (!claim(source)) return null
        val stream = AtomicReference<InputStream?>()
        val abandoned = AtomicBoolean(false)
        val task = FutureTask<ByteArray?> {
            try {
                val input = open() ?: return@FutureTask null
                stream.set(input)
                // The caller may have stopped waiting while this was opening.
                input.use { if (abandoned.get()) null else readAtMost(it, maxBytes) }
            } finally {
                release(source)
            }
        }
        try {
            threads.execute(task)
        } catch (_: RejectedExecutionException) {
            release(source)
            return null
        }
        return try {
            task.get(deadlineMillis, TimeUnit.MILLISECONDS)
        } catch (_: TimeoutException) {
            abandoned.set(true)
            stream.get()?.let { runCatching { it.close() } }
            null
        } catch (_: ExecutionException) {
            null
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        }
    }

    private fun claim(source: String): Boolean = synchronized(running) {
        val forSource = running[source] ?: 0
        if (total >= maxReads || forSource >= maxPerSource) return false
        running[source] = forSource + 1
        total++
        true
    }

    private fun release(source: String) = synchronized(running) {
        val left = (running[source] ?: 1) - 1
        if (left == 0) running.remove(source) else running[source] = left
        total--
    }
}

/** All of [input], or null if it holds more than [max] bytes. */
internal fun readAtMost(input: InputStream, max: Int): ByteArray? {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(16 * 1024)
    while (true) {
        val read = input.read(buffer)
        if (read < 0) return out.toByteArray()
        out.write(buffer, 0, read)
        if (out.size() > max) return null
    }
}
