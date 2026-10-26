package dev.link2linux.android.core.files

/**
 * The rules for file names (docs/protocol.md, "Files").
 *
 * A name on the wire is the file's name without any directory: 1 to 255
 * bytes of UTF-8, no "/", no NUL, and neither "." nor "..". A received name
 * is never used as a path. [safe] turns it into a name the phone can store
 * and show without surprises, and [numbered] gives the names tried when
 * that one is taken, since a received file never replaces another.
 */
object FileNames {

    /** The longest name, in bytes of UTF-8, on the wire and on the phone's storage. */
    const val MAX_BYTES = 255

    /** What a name with nothing usable in it becomes. */
    const val FALLBACK = "file"

    /** Whether [name] follows the wire rules. A receiver refuses an offer with any other name. */
    fun isValid(name: String): Boolean =
        name.isNotEmpty() && utf8Length(name) <= MAX_BYTES && '/' !in name && '\u0000' !in name && name != "." && name != ".."

    /**
     * [name] made safe to store and to show: path separators, control
     * characters, the invisible characters that change the direction of
     * text (they can make `exe.jpg` read as `gpj.exe`), and the characters
     * shared storage cannot hold become `_`. Leading dots, which would hide
     * the file, become `_` as well, and spaces around the name go. The
     * result fits [MAX_BYTES] without splitting a character, keeping a short
     * extension. A name with nothing left becomes [fallback].
     *
     * The sender uses it too, since the names of the phone's documents
     * follow no rules of ours.
     */
    fun safe(name: String, fallback: String = FALLBACK): String {
        val replaced = buildString(name.length) {
            var i = 0
            while (i < name.length) {
                val cp = name.codePointAt(i)
                if (isUnsafe(cp)) append('_') else appendCodePoint(cp)
                i += Character.charCount(cp)
            }
        }.trim()
        val dots = replaced.takeWhile { it == '.' }.length
        val visible = "_".repeat(dots) + replaced.substring(dots)
        return fit(visible, MAX_BYTES).ifEmpty { fallback }
    }

    /**
     * The [n]th name to try after [name] (a [safe] one) is taken: `photo (1).jpg`
     * for `photo.jpg`, still within [MAX_BYTES].
     */
    fun numbered(name: String, n: Int): String {
        require(n > 0) { "Numbers start at 1" }
        val (base, extension) = split(name)
        val suffix = " ($n)$extension"
        return fit(base, MAX_BYTES - utf8Length(suffix)) + suffix
    }

    /** The number of bytes [text] takes in UTF-8. */
    fun utf8Length(text: String): Int {
        var bytes = 0
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            bytes += utf8Length(cp)
            i += Character.charCount(cp)
        }
        return bytes
    }

    /** [name] cut to [maxBytes], keeping its extension when that is short enough to matter. */
    private fun fit(name: String, maxBytes: Int): String {
        if (utf8Length(name) <= maxBytes) return name
        val (base, extension) = split(name)
        val extensionBytes = utf8Length(extension)
        return if (extension.isNotEmpty() && extensionBytes <= MAX_EXTENSION_BYTES && extensionBytes < maxBytes) {
            prefix(base, maxBytes - extensionBytes) + extension
        } else {
            prefix(name, maxBytes)
        }
    }

    /** The longest start of [text] that fits [maxBytes], ending on a character boundary. */
    private fun prefix(text: String, maxBytes: Int): String {
        var bytes = 0
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            val size = utf8Length(cp)
            if (bytes + size > maxBytes) break
            bytes += size
            i += Character.charCount(cp)
        }
        return text.substring(0, i)
    }

    /** The name before its extension, and the extension with its dot (empty when there is none). */
    private fun split(name: String): Pair<String, String> {
        val dot = name.lastIndexOf('.')
        return if (dot <= 0) name to "" else name.substring(0, dot) to name.substring(dot)
    }

    private fun utf8Length(cp: Int): Int = when {
        cp < 0x80 -> 1
        cp < 0x800 -> 2
        cp < 0x10000 -> 3
        else -> 4
    }

    private fun isUnsafe(cp: Int): Boolean =
        Character.isISOControl(cp) || (cp < 0x80 && cp.toChar() in RESERVED) || cp in DIRECTION_CONTROLS ||
            // A lone surrogate, which no file system can name a file with.
            Character.getType(cp) == Character.SURROGATE.toInt()

    /** Separators, and what FAT-formatted shared storage refuses. */
    private const val RESERVED = "/\\\"*:<>?|"

    /** Marks and overrides that change how the text around them is ordered. */
    private val DIRECTION_CONTROLS: Set<Int> =
        setOf(0x061C, 0x200E, 0x200F) + (0x202A..0x202E) + (0x2066..0x2069)

    /** Longer "extensions" are part of the name, and are cut like it. */
    private const val MAX_EXTENSION_BYTES = 16
}
