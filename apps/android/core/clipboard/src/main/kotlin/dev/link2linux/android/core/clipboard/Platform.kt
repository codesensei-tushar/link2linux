package dev.link2linux.android.core.clipboard

/** The most text one push carries, in UTF-8 bytes (docs/protocol.md, "Clipboard"). */
const val MAX_TEXT_BYTES: Int = 256 * 1024

/**
 * Text for or from a clipboard. [sensitive] marks text the source app flagged
 * as secret, which clipboard history and copy previews leave out.
 *
 * [toString] gives only the length, so the text never reaches a log.
 */
data class ClipText(val text: String, val sensitive: Boolean = false) {
    override fun toString() = "ClipText(${text.length} chars, sensitive=$sensitive)"
}

/** The phone's clipboard, as the engine writes it. */
fun interface ClipboardWriter {
    /** Puts [clip] on the clipboard, replacing what is there. Throws when the platform refuses. */
    suspend fun write(clip: ClipText)
}
