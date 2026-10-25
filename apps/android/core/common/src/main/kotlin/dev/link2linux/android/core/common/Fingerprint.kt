package dev.link2linux.android.core.common

import java.security.MessageDigest

/**
 * Certificate fingerprints as pairing stores them: lowercase hex SHA-256 of
 * the DER-encoded certificate. Later sessions compare against this value.
 */
object Fingerprint {
    fun sha256Hex(der: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(der).toHex()

    /** Constant-time comparison, so a mismatch does not leak how many characters matched. */
    fun matches(expectedHex: String, der: ByteArray): Boolean = MessageDigest.isEqual(
        expectedHex.lowercase().toByteArray(Charsets.US_ASCII),
        sha256Hex(der).toByteArray(Charsets.US_ASCII),
    )
}

private const val HEX_DIGITS = "0123456789abcdef"

/** Lowercase hex, two characters per byte. */
fun ByteArray.toHex(): String = buildString(size * 2) {
    for (byte in this@toHex) {
        val value = byte.toInt() and 0xff
        append(HEX_DIGITS[value ushr 4])
        append(HEX_DIGITS[value and 0x0f])
    }
}
