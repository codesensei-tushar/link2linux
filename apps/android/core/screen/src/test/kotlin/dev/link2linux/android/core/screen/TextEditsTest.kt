package dev.link2linux.android.core.screen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TextEditsTest {

    @Test
    fun typingGoesInAtTheCursor() {
        assertEquals(TextEdits.Edit("heXllo", 3), TextEdits.insert("hello", 2, 2, "X"))
    }

    @Test
    fun typingReplacesTheSelectionInWhicheverOrderItIsGiven() {
        assertEquals(TextEdits.Edit("hXo", 2), TextEdits.insert("hello", 1, 4, "X"))
        assertEquals(TextEdits.Edit("hXo", 2), TextEdits.insert("hello", 4, 1, "X"))
    }

    @Test
    fun aFieldThatDoesNotSayWhereItsCursorIsGetsTheTextAtTheEnd() {
        assertEquals(TextEdits.Edit("hello!", 6), TextEdits.insert("hello", -1, -1, "!"))
        // A selection past the end, as from a field whose text changed meanwhile, stops at the end.
        assertEquals(TextEdits.Edit("hello!", 6), TextEdits.insert("hello", 9, 9, "!"))
    }

    @Test
    fun backspaceDeletesTheCharacterBeforeTheCursor() {
        assertEquals(TextEdits.Edit("helo", 2), TextEdits.backspace("hello", 3, 3))
        assertEquals(TextEdits.Edit("hell", 4), TextEdits.backspace("hello", -1, -1))
    }

    @Test
    fun backspaceDeletesAWholeSurrogatePair() {
        val text = "a" + String(Character.toChars(0x1F600)) // an emoji outside the BMP: a surrogate pair

        assertEquals(TextEdits.Edit("a", 1), TextEdits.backspace(text, 3, 3))
    }

    @Test
    fun backspaceDeletesTheSelection() {
        assertEquals(TextEdits.Edit("ho", 1), TextEdits.backspace("hello", 1, 4))
    }

    @Test
    fun backspaceAtTheStartDeletesNothing() {
        assertNull(TextEdits.backspace("hello", 0, 0))
        assertNull(TextEdits.backspace("", -1, -1))
    }

    @Test
    fun selectsAllOnlyWhenTheWholeTextIsSelected() {
        assertTrue(TextEdits.selectsAll("abc", 0, 3))
        assertTrue(TextEdits.selectsAll("abc", 3, 0))
        assertFalse(TextEdits.selectsAll("abc", 1, 3))
        assertFalse(TextEdits.selectsAll("abc", -1, -1))
    }
}
