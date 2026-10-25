package dev.link2linux.android.core.model

import java.util.Base64

/**
 * `fp(X)` in the protocol: the SHA-256 of a device's public key, taken over
 * the DER encoding of its certificate's SubjectPublicKeyInfo. Devices
 * recognize each other by this value alone.
 */
class KeyFingerprint private constructor(private val bytes: ByteArray) {

    /** A copy of the 32 bytes. */
    fun toByteArray(): ByteArray = bytes.copyOf()

    /** Lowercase hex, 64 characters. */
    fun toHex(): String = buildString(SIZE * 2) {
        for (byte in bytes) {
            val value = byte.toInt() and 0xff
            append(HEX_DIGITS[value ushr 4])
            append(HEX_DIGITS[value and 0x0f])
        }
    }

    /** base64url without padding, 43 characters, as in the QR code's `f`. */
    fun toBase64Url(): String = Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)

    override fun equals(other: Any?): Boolean = other is KeyFingerprint && bytes.contentEquals(other.bytes)

    override fun hashCode(): Int = bytes.contentHashCode()

    override fun toString(): String = "KeyFingerprint(${toHex()})"

    companion object {
        /** Length of a fingerprint in bytes. */
        const val SIZE: Int = 32

        private const val HEX_DIGITS = "0123456789abcdef"

        /** @throws IllegalArgumentException unless [bytes] holds exactly 32 bytes. */
        fun of(bytes: ByteArray): KeyFingerprint {
            require(bytes.size == SIZE) { "A fingerprint has $SIZE bytes, not ${bytes.size}" }
            return KeyFingerprint(bytes.copyOf())
        }

        /** Parses 64 hex digits, in either case. */
        fun fromHex(hex: String): KeyFingerprint {
            require(hex.length == SIZE * 2) { "A fingerprint has ${SIZE * 2} hex digits, not ${hex.length}" }
            return KeyFingerprint(ByteArray(SIZE) { i -> hex.substring(i * 2, i * 2 + 2).toInt(16).toByte() })
        }

        /** Parses unpadded base64url, as in the QR code's `f`. */
        fun fromBase64Url(text: String): KeyFingerprint {
            require('=' !in text) { "base64url without padding expected" }
            return of(Base64.getUrlDecoder().decode(text))
        }
    }
}
