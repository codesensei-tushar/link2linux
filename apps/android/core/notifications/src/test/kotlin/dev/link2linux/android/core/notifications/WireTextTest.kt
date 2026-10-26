package dev.link2linux.android.core.notifications

import java.util.Base64
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class WireTextTest {
    @Test
    fun `identifier up to 256 bytes is kept`() {
        val key = "a".repeat(256)
        assertEquals(key, WireText.identifier(key))
    }

    @Test
    fun `identifier over 256 bytes becomes h and the base64url sha256`() {
        val key = "0|pkg|1|" + "x".repeat(300)
        val id = WireText.identifier(key)
        assertEquals(45, id.length)
        assertTrue(id.startsWith("h:"))
        val digest = WireText.sha256(key.toByteArray())
        assertEquals(Base64.getUrlEncoder().withoutPadding().encodeToString(digest), id.removePrefix("h:"))
    }

    @Test
    fun `identifier counts bytes not characters`() {
        val key = "é".repeat(129)
        assertTrue(WireText.identifier(key).startsWith("h:"))
    }

    @Test
    fun `truncate respects code point boundaries`() {
        val text = "ab😀cd"
        assertEquals("ab", WireText.truncate(text, 5))
        assertEquals("ab😀", WireText.truncate(text, 6))
        assertEquals(text, WireText.truncate(text, 100))
    }
}
