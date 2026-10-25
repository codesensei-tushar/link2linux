package dev.link2linux.android.core.model

/**
 * Fixed-size secret bytes. [toString] never shows them, so a secret cannot
 * end up in a log by accident, and equality compares the content.
 */
sealed class SecretBytes(private val bytes: ByteArray) {

    /** A copy of the secret's bytes. */
    fun toByteArray(): ByteArray = bytes.copyOf()

    override fun equals(other: Any?): Boolean =
        other != null && other::class == this::class && bytes.contentEquals((other as SecretBytes).bytes)

    override fun hashCode(): Int = bytes.contentHashCode()

    override fun toString(): String = "${this::class.simpleName}(redacted)"
}

/** The one-time secret from a desktop's QR code: 16 random bytes. */
class PairingSecret private constructor(bytes: ByteArray) : SecretBytes(bytes) {
    companion object {
        /** Length of a secret in bytes. */
        const val SIZE: Int = 16

        /** @throws IllegalArgumentException unless [bytes] holds exactly 16 bytes. */
        fun of(bytes: ByteArray): PairingSecret {
            require(bytes.size == SIZE) { "A pairing secret has $SIZE bytes, not ${bytes.size}" }
            return PairingSecret(bytes.copyOf())
        }
    }
}

/**
 * The key behind the phone's presence tags for one desktop: 32 random bytes
 * that the phone creates when it pairs and sends to that desktop only, in
 * `PresenceUpdate`. Removing the pairing drops it.
 */
class PresenceKey private constructor(bytes: ByteArray) : SecretBytes(bytes) {
    companion object {
        /** Length of a presence key in bytes. */
        const val SIZE: Int = 32

        /** @throws IllegalArgumentException unless [bytes] holds exactly 32 bytes. */
        fun of(bytes: ByteArray): PresenceKey {
            require(bytes.size == SIZE) { "A presence key has $SIZE bytes, not ${bytes.size}" }
            return PresenceKey(bytes.copyOf())
        }
    }
}
