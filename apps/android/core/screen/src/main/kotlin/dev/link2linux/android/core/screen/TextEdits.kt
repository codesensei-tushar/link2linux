package dev.link2linux.android.core.screen

import java.text.BreakIterator
import kotlin.math.max
import kotlin.math.min

/**
 * What typing does to a text field's content. Android lets the
 * accessibility service only replace a field's whole text, so typing reads
 * the text and its selection, edits them here, and writes the result back.
 */
internal object TextEdits {

    /** A field's new content, and where its cursor goes. */
    data class Edit(val text: String, val cursor: Int)

    /** [typed] at the cursor, in place of the selection. */
    fun insert(text: String, selectionStart: Int, selectionEnd: Int, typed: String): Edit {
        val (from, to) = selection(text, selectionStart, selectionEnd)
        return Edit(text.substring(0, from) + typed + text.substring(to), from + typed.length)
    }

    /** Deletes the selection, or else the character before the cursor; null when there is nothing to delete. */
    fun backspace(text: String, selectionStart: Int, selectionEnd: Int): Edit? {
        val (from, to) = selection(text, selectionStart, selectionEnd)
        if (from != to) return Edit(text.removeRange(from, to), from)
        if (from == 0) return null
        // A character as the user sees it, which can be several chars: a surrogate pair, or a letter and its accents.
        val characters = BreakIterator.getCharacterInstance().apply { setText(text) }
        val start = characters.preceding(from).takeIf { it != BreakIterator.DONE } ?: (from - 1)
        return Edit(text.removeRange(start, from), start)
    }

    /** Whether the selection takes in the whole of [text]. */
    fun selectsAll(text: String, selectionStart: Int, selectionEnd: Int): Boolean {
        val (from, to) = selection(text, selectionStart, selectionEnd)
        return from == 0 && to == text.length
    }

    /** The selection in order and within [text]; a field that does not say has its cursor at the end. */
    private fun selection(text: String, start: Int, end: Int): Pair<Int, Int> {
        if (start < 0 || end < 0) return text.length to text.length
        return min(start, end).coerceAtMost(text.length) to max(start, end).coerceAtMost(text.length)
    }
}
