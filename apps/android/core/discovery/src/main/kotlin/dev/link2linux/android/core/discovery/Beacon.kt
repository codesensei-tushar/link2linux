package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PresenceKey
import java.nio.ByteBuffer

/**
 * The 40-byte datagram the phone sends to a desktop's beacon port. Each
 * beacon is keyed for one desktop: with that desktop's presence key, or with
 * the secret of the QR code the phone scanned while it waits for the desktop.
 */
object Beacon {

    const val SIZE: Int = 40
    const val NONCE_SIZE: Int = 8

    private const val VERSION: Byte = 1
    private const val FLAG_PAIRING: Byte = 1
    private val magic = "L2LB".toByteArray(Charsets.US_ASCII)
    private val label = "l2l-beacon-v1".toByteArray(Charsets.US_ASCII)

    /** A beacon for a paired desktop. */
    fun presence(key: PresenceKey, port: Int, unixSeconds: Long, nonce: ByteArray): ByteArray =
        encode(key.toByteArray(), pairing = false, port, unixSeconds, nonce)

    /** A beacon for the desktop that showed the QR code with [secret]. */
    fun pairing(secret: PairingSecret, port: Int, unixSeconds: Long, nonce: ByteArray): ByteArray =
        encode(secret.toByteArray(), pairing = true, port, unixSeconds, nonce)

    private fun encode(key: ByteArray, pairing: Boolean, port: Int, unixSeconds: Long, nonce: ByteArray): ByteArray {
        require(port in 1..65535) { "Not a TCP port: $port" }
        require(nonce.size == NONCE_SIZE) { "A beacon nonce has $NONCE_SIZE bytes" }
        val header = ByteBuffer.allocate(24)
            .put(magic)
            .put(VERSION)
            .put(if (pairing) FLAG_PAIRING else 0)
            .putShort(port.toShort())
            .putLong(unixSeconds)
            .put(nonce)
            .array()
        val tag = hmacSha256(key, label, header).copyOf(16)
        return header + tag
    }
}
