package dev.link2linux.android.core.common

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FingerprintTest {

    @Test
    fun sha256MatchesKnownVectors() {
        assertEquals(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            Fingerprint.sha256Hex(ByteArray(0)),
        )
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            Fingerprint.sha256Hex("abc".toByteArray()),
        )
    }

    @Test
    fun hexKeepsLeadingZerosAndHighBytes() {
        assertEquals("000fff80", byteArrayOf(0x00, 0x0f, 0xff.toByte(), 0x80.toByte()).toHex())
    }

    @Test
    fun matchesIgnoresHexCase() {
        val der = "certificate".toByteArray()
        val expected = Fingerprint.sha256Hex(der)
        assertTrue(Fingerprint.matches(expected, der))
        assertTrue(Fingerprint.matches(expected.uppercase(), der))
        assertFalse(Fingerprint.matches(expected, "other".toByteArray()))
    }
}
