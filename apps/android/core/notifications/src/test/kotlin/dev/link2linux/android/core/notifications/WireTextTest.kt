package dev.link2linux.android.core.notifications

import org.junit.Assert.assertEquals
import org.junit.Test

class WireTextTest {
    @Test
    fun `truncate respects code point boundaries`() {
        val text = "ab😀cd"
        assertEquals("ab", WireText.truncate(text, 5))
        assertEquals("ab😀", WireText.truncate(text, 6))
        assertEquals(text, WireText.truncate(text, 100))
    }
}
