package dev.link2linux.android.core.status

import dev.link2linux.protocol.v1.PhoneStatus
import kotlinx.coroutines.flow.Flow

/** The most `Cellular.operator_name` carries, in UTF-8 bytes (docs/protocol.md, "Phone status"). */
const val MAX_OPERATOR_NAME_BYTES: Int = 64

/** The highest signal level, as the status bar's bars count it. */
const val MAX_SIGNAL_LEVEL: Int = 4

/** The phone's battery, signal and sound mode, as the platform reports them. */
fun interface StatusSource {
    /**
     * The phone's complete status: the current one at once, then a new one
     * after each change. Collecting starts the platform's listeners, and
     * cancelling stops them. A part the platform does not tell without a
     * permission stays unset.
     */
    fun statuses(): Flow<PhoneStatus>
}

/**
 * [text] cut to at most [maxBytes] of UTF-8 at a character boundary, so no
 * character is split. A lone surrogate counts as three bytes, more than the
 * one byte it is sent as, which only ever cuts earlier.
 */
fun truncateUtf8(text: String, maxBytes: Int): String {
    var bytes = 0
    var i = 0
    while (i < text.length) {
        val cp = text.codePointAt(i)
        bytes += when {
            cp < 0x80 -> 1
            cp < 0x800 -> 2
            cp < 0x10000 -> 3
            else -> 4
        }
        if (bytes > maxBytes) break
        i += Character.charCount(cp)
    }
    return text.substring(0, i)
}
