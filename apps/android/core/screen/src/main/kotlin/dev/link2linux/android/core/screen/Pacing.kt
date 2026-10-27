package dev.link2linux.android.core.screen

import kotlin.math.max
import kotlin.math.min

/**
 * Keeps a stream at [perSecond] frames a second at most, by the frames'
 * capture times. Up to [burst] frames may come closer together, so that a
 * frame captured a little early, as at a display's 60 Hz, is not dropped.
 */
internal class FrameRate(private val perSecond: Int = MAX_PER_SECOND, private val burst: Int = 2) {
    private var tokens = burst.toDouble()
    private var lastUs: Long? = null

    /** Whether a frame captured at [timestampUs] may go out. */
    fun take(timestampUs: Long): Boolean {
        lastUs?.let { tokens = min(burst.toDouble(), tokens + max(0, timestampUs - it) * perSecond / 1e6) }
        lastUs = timestampUs
        if (tokens < 1) return false
        tokens -= 1
        return true
    }

    companion object {
        /** docs/protocol.md, "Video". */
        const val MAX_PER_SECOND = 60
    }
}

/**
 * The share of the full bitrate, or JPEG quality, that a stream encodes
 * at. A network that cannot keep up, or a frame too large to send, lowers
 * it at once, at most once a second; [RAISE_AFTER_US] without trouble
 * raise it again step by step. Times are the frames' capture times.
 */
internal class Quality {
    var value = 1f
        private set

    private var loweredAtUs: Long? = null
    private var changedAtUs: Long? = null

    /** Lowers the quality; returns the new one, or null when it stays as it is. */
    fun lower(timestampUs: Long): Float? {
        loweredAtUs?.let { if (timestampUs - it < LOWER_EVERY_US) return null }
        if (value <= MIN) return null
        value = max(MIN, value * LOWER_BY)
        loweredAtUs = timestampUs
        changedAtUs = timestampUs
        return value
    }

    /** A frame went out; returns a higher quality once it has been fine for a while, else null. */
    fun fine(timestampUs: Long): Float? {
        if (value >= 1f) return null
        val since = changedAtUs ?: timestampUs.also { changedAtUs = it }
        if (timestampUs - since < RAISE_AFTER_US) return null
        value = min(1f, value * RAISE_BY)
        changedAtUs = timestampUs
        return value
    }

    companion object {
        const val MIN = 0.1f
        private const val LOWER_BY = 0.7f
        private const val RAISE_BY = 1.25f
        private const val LOWER_EVERY_US = 1_000_000L
        const val RAISE_AFTER_US = 5_000_000L
    }
}
