package dev.link2linux.android.core.screen

import dev.link2linux.protocol.v1.ScreenFormat
import dev.link2linux.protocol.v1.ScreenFrame
import kotlinx.coroutines.channels.Channel

/**
 * One stream's messages on their way to the desktop, in order, with the
 * flow control of docs/protocol.md, "Video": no more than [maxWaitingUs] of
 * video waits to be written, counting the frame being written.
 *
 * The encoder offers frames on its own thread, and one sender takes them
 * one at a time ([next], then [written] once the session wrote it), so a
 * slow network shows here as frames that wait. A frame that would make the
 * wait too long is dropped, and so is every frame after it up to the next
 * key frame, since an H.264 frame cannot be decoded without the ones
 * before it. A key frame needs nothing before it: it takes the place of
 * the frames still waiting, so after a stall the desktop catches up at
 * once. Every JPEG frame is a key frame, so with JPEG only the newest
 * frame waits.
 */
internal class Outbox(private val maxWaitingUs: Long = MAX_WAITING_US) {

    sealed interface Item {
        class Format(val format: ScreenFormat) : Item

        class Frame(val frame: ScreenFrame) : Item
    }

    /** What became of an offered frame. */
    enum class Offer {
        QUEUED,

        /** Dropped because too much waits; the frames after it wait for a key frame, so ask for one. */
        DROPPED,

        /** Dropped because the frames wait for a key frame. */
        WAITING,

        /** Dropped because the stream ended. */
        CLOSED,
    }

    private val lock = Any()
    private val items = ArrayDeque<Item>() // guarded by lock
    private var writing: Long? = null // the timestamp of the frame being written; guarded by lock
    private var needsKeyFrame = false // guarded by lock
    private var closed = false // guarded by lock
    private val ready = Channel<Unit>(Channel.CONFLATED)

    /**
     * Queues [format]. A format of new video ([newVideo]) makes the frames
     * still waiting useless, and those after it wait for its key frame; one
     * that only changes `input` goes out in order behind them.
     */
    fun format(format: ScreenFormat, newVideo: Boolean) {
        synchronized(lock) {
            if (closed) return
            if (newVideo) {
                items.clear()
                needsKeyFrame = true
            }
            items.addLast(Item.Format(format))
        }
        ready.trySend(Unit)
    }

    fun frame(frame: ScreenFrame): Offer {
        synchronized(lock) {
            when {
                closed -> return Offer.CLOSED
                frame.keyFrame -> {
                    items.removeAll { it is Item.Frame }
                    needsKeyFrame = false
                }
                needsKeyFrame -> return Offer.WAITING
                waitingSince()?.let { frame.timestampUs - it > maxWaitingUs } == true -> {
                    needsKeyFrame = true
                    return Offer.DROPPED
                }
            }
            items.addLast(Item.Frame(frame))
        }
        ready.trySend(Unit)
        return Offer.QUEUED
    }

    /** Drops the frames from now on up to the next key frame; those already waiting still go out. */
    fun skipToKeyFrame() = synchronized(lock) { needsKeyFrame = true }

    /** The next message to write, once there is one, or null once the stream ended. Call [written] after writing it. */
    suspend fun next(): Item? {
        while (true) {
            synchronized(lock) {
                if (closed) return null
                val item = items.removeFirstOrNull()
                if (item != null) {
                    if (item is Item.Frame) writing = item.frame.timestampUs
                    return item
                }
            }
            ready.receive()
        }
    }

    fun written() = synchronized(lock) { writing = null }

    /** Ends the stream: drops what waits, and [next] returns null. */
    fun close() {
        synchronized(lock) {
            closed = true
            items.clear()
        }
        ready.trySend(Unit)
    }

    private fun waitingSince(): Long? =
        writing ?: items.firstNotNullOfOrNull { (it as? Item.Frame)?.frame?.timestampUs }

    companion object {
        /** About half a second (docs/protocol.md, "Video"). */
        const val MAX_WAITING_US: Long = 500_000
    }
}
