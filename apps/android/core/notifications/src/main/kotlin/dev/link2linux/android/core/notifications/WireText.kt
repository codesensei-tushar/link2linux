package dev.link2linux.android.core.notifications

import java.security.MessageDigest
import java.util.Base64

/** Size limits of docs/protocol.md, "Limits". Bytes of UTF-8 unless noted. */
object Limits {
    const val KEY = 256
    const val APP_NAME = 256
    const val SENDER = 256
    const val SHORT_TEXT = 1024
    const val LONG_TEXT = 8 * 1024
    const val MESSAGES = 10
    const val ACTIONS = 8
    const val REPLY_CHOICES = 5
    const val REPLY_CHOICE = 256
    const val REPLY_TEXT = 8 * 1024
    const val SYNC_BATCH_NOTIFICATIONS = 100
    const val SYNC_BATCH_BYTES = 256 * 1024
    const val IMAGE_BYTES = 1024 * 1024
}

object WireText {
    private val base64Url = Base64.getUrlEncoder().withoutPadding()

    fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

    /** Cuts at a character boundary so the UTF-8 form fits [maxBytes]. */
    fun truncate(value: String, maxBytes: Int): String {
        if (value.length * 3 <= maxBytes || value.toByteArray(Charsets.UTF_8).size <= maxBytes) return value
        var bytes = 0
        var end = 0
        while (end < value.length) {
            val cp = value.codePointAt(end)
            val size = when {
                cp < 0x80 -> 1
                cp < 0x800 -> 2
                cp < 0x10000 -> 3
                else -> 4
            }
            if (bytes + size > maxBytes) break
            bytes += size
            end += Character.charCount(cp)
        }
        return value.substring(0, end)
    }

    /** Short, stable text for action ids. */
    fun shortHash(vararg parts: String): String {
        val digest = MessageDigest.getInstance("SHA-256")
        for (part in parts) {
            digest.update(part.toByteArray(Charsets.UTF_8))
            digest.update(0)
        }
        return base64Url.encodeToString(digest.digest().copyOf(9))
    }
}
