package dev.link2linux.android.core.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FileNamesTest {

    @Test
    fun wireNamesFollowTheProtocolRules() {
        assertTrue(FileNames.isValid("photo.jpg"))
        assertTrue(FileNames.isValid(".bashrc"))
        assertTrue(FileNames.isValid("a".repeat(255)))
        // 85 characters of three bytes each: 255 bytes.
        assertTrue(FileNames.isValid("€".repeat(85)))
        assertFalse(FileNames.isValid(""))
        assertFalse(FileNames.isValid("a".repeat(256)))
        assertFalse(FileNames.isValid("€".repeat(86)))
        assertFalse(FileNames.isValid("dir/photo.jpg"))
        assertFalse(FileNames.isValid("photo\u0000.jpg"))
        assertFalse(FileNames.isValid("."))
        assertFalse(FileNames.isValid(".."))
    }

    @Test
    fun separatorsAndControlCharactersBecomeUnderscores() {
        assertEquals("a_b_c", FileNames.safe("a/b\\c"))
        assertEquals("a_b_c_d", FileNames.safe("a\u0000b\nc\u007fd"))
        assertEquals("what_ _ _ _ _ _ _", FileNames.safe("what? : * < > | \""))
    }

    @Test
    fun directionControlsCannotDisguiseAnExtension() {
        // With the override in place, "photo\u202Egpj.exe" reads as "photoexe.jpg".
        assertEquals("photo_gpj.exe", FileNames.safe("photo\u202Egpj.exe"))
        assertEquals("a_b_c_d", FileNames.safe("a\u200Eb\u2066c\u061Cd"))
    }

    @Test
    fun aNameCannotBeHiddenOrClimbOut() {
        assertEquals("_bashrc", FileNames.safe(".bashrc"))
        assertEquals("_", FileNames.safe("."))
        assertEquals("__", FileNames.safe(".."))
        assertEquals("___secret", FileNames.safe("...secret"))
        assertEquals("___etc_passwd", FileNames.safe("../etc/passwd"))
    }

    @Test
    fun emptyNamesGetTheFallback() {
        assertEquals(FileNames.FALLBACK, FileNames.safe(""))
        assertEquals(FileNames.FALLBACK, FileNames.safe("   "))
        assertEquals("trimmed.txt", FileNames.safe("  trimmed.txt  "))
    }

    @Test
    fun longNamesAreCutOnACharacterBoundaryKeepingTheExtension() {
        val long = "€".repeat(100) + ".jpg"
        val safe = FileNames.safe(long)
        assertTrue(FileNames.utf8Length(safe) <= 255)
        assertTrue(safe.endsWith(".jpg"))
        // 251 bytes are left for the name, room for 83 whole euro signs.
        assertEquals("€".repeat(83) + ".jpg", safe)

        // Characters outside the BMP are never split into halves.
        val emoji = "😀".repeat(70) // 280 bytes
        val cut = FileNames.safe(emoji)
        assertEquals("😀".repeat(63), cut)
        assertTrue(cut.codePoints().allMatch { it == "😀".codePointAt(0) })

        // A long "extension" is part of the name.
        val dotted = "a." + "b".repeat(300)
        assertEquals(255, FileNames.utf8Length(FileNames.safe(dotted)))
    }

    @Test
    fun loneSurrogatesAreReplaced() {
        assertEquals("a_b", FileNames.safe("a\uD800b"))
    }

    @Test
    fun numberedNamesGoBeforeTheExtension() {
        assertEquals("photo (1).jpg", FileNames.numbered("photo.jpg", 1))
        assertEquals("photo (12).jpg", FileNames.numbered("photo.jpg", 12))
        assertEquals("README (2)", FileNames.numbered("README", 2))
        assertEquals("archive.tar (1).gz", FileNames.numbered("archive.tar.gz", 1))
        assertEquals("_hidden (1)", FileNames.numbered("_hidden", 1))
    }

    @Test
    fun numberedNamesStayWithinTheLimit() {
        val name = FileNames.safe("x".repeat(251) + ".jpg")
        assertEquals(255, FileNames.utf8Length(name))
        val numbered = FileNames.numbered(name, 3)
        assertEquals(255, FileNames.utf8Length(numbered))
        assertTrue(numbered.endsWith(" (3).jpg"))
    }
}
