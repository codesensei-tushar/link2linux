package dev.link2linux.android.core.model

/**
 * A name another device sent, without the characters that change how it
 * looks while staying unseen themselves: controls, line and paragraph
 * separators, and format characters such as zero-width spaces, byte order
 * marks and bidirectional controls. The joiners stay, as emoji sequences
 * and some scripts need them. Names are shown in the prompts the user
 * decides on, so they must look like what they are.
 */
fun withoutHiddenCharacters(name: String): String = buildString {
    name.codePoints().forEach { if (!isHidden(it)) appendCodePoint(it) }
}

private fun isHidden(codePoint: Int): Boolean = when (Character.getType(codePoint).toByte()) {
    Character.CONTROL, Character.LINE_SEPARATOR, Character.PARAGRAPH_SEPARATOR -> true
    Character.FORMAT -> codePoint != ZERO_WIDTH_NON_JOINER && codePoint != ZERO_WIDTH_JOINER
    else -> false
}

private const val ZERO_WIDTH_NON_JOINER = 0x200C
private const val ZERO_WIDTH_JOINER = 0x200D
