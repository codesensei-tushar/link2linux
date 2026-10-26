package dev.link2linux.android.core.security

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.SasCode
import java.security.MessageDigest
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * The pairing constructions of the protocol (docs/protocol.md, "Pairing").
 * Every one of them is checked against proto/testdata/v1/pairing.txt.
 *
 * `ekm` is the value both sides export from their TLS connection with
 * [QR_EXPORTER_LABEL] or [SAS_EXPORTER_LABEL] and [exporterContext]. It
 * never travels over the network.
 */
object PairingCrypto {

    /** Exporter label for QR code pairing. */
    const val QR_EXPORTER_LABEL: String = "EXPERIMENTAL-link2linux-pair-v1"

    /** Exporter label for SAS pairing. */
    const val SAS_EXPORTER_LABEL: String = "EXPERIMENTAL-link2linux-sas-v1"

    /** Length of the exported value, in bytes. */
    const val EKM_SIZE: Int = 32

    /** Length of each SAS nonce, in bytes. */
    const val NONCE_SIZE: Int = 32

    /** Length of a `PairingProof` MAC and of an `SasCommit` commitment, in bytes. */
    const val TAG_SIZE: Int = 32

    /** Who sends a `PairingProof`. The byte goes into the MAC, so a proof cannot be reflected. */
    enum class Role(val byte: Byte) { DESKTOP(0x01), PHONE(0x02) }

    /** The exporter context: `fp_D || fp_P`, 64 bytes, never absent. */
    fun exporterContext(desktop: KeyFingerprint, phone: KeyFingerprint): ByteArray =
        desktop.toByteArray() + phone.toByteArray()

    /** `HMAC-SHA256(secret, "l2l-pair-v1" || role || ekm)`. */
    fun proof(secret: PairingSecret, role: Role, ekm: ByteArray): ByteArray {
        requireEkm(ekm)
        return hmac(secret.toByteArray(), ascii("l2l-pair-v1") + role.byte + ekm)
    }

    /** Whether [mac] is [role]'s proof, compared in constant time. */
    fun verifyProof(secret: PairingSecret, role: Role, ekm: ByteArray, mac: ByteArray): Boolean =
        MessageDigest.isEqual(proof(secret, role, ekm), mac)

    /** The phone's commitment to its nonce: `SHA-256("l2l-sas-commit-v1" || ekm || n_p)`. */
    fun sasCommitment(ekm: ByteArray, phoneNonce: ByteArray): ByteArray {
        requireEkm(ekm)
        requireNonce(phoneNonce)
        return MessageDigest.getInstance("SHA-256").digest(ascii("l2l-sas-commit-v1") + ekm + phoneNonce)
    }

    /** The code both screens show: the first 4 bytes of [sasHmac], big-endian, mod 1000000. */
    fun sasCode(ekm: ByteArray, desktopNonce: ByteArray, phoneNonce: ByteArray): SasCode {
        val h = sasHmac(ekm, desktopNonce, phoneNonce)
        val value = (h[0].toLong() and 0xff shl 24) or (h[1].toLong() and 0xff shl 16) or
            (h[2].toLong() and 0xff shl 8) or (h[3].toLong() and 0xff)
        return SasCode((value % 1_000_000).toInt())
    }

    /** `HMAC-SHA256(ekm, "l2l-sas-v1" || n_d || n_p)`. */
    internal fun sasHmac(ekm: ByteArray, desktopNonce: ByteArray, phoneNonce: ByteArray): ByteArray {
        requireEkm(ekm)
        requireNonce(desktopNonce)
        requireNonce(phoneNonce)
        return hmac(ekm, ascii("l2l-sas-v1") + desktopNonce + phoneNonce)
    }

    private fun hmac(key: ByteArray, message: ByteArray): ByteArray =
        Mac.getInstance("HmacSHA256").run {
            init(SecretKeySpec(key, "HmacSHA256"))
            doFinal(message)
        }

    private fun ascii(text: String): ByteArray = text.toByteArray(Charsets.US_ASCII)

    private fun requireEkm(ekm: ByteArray) =
        require(ekm.size == EKM_SIZE) { "Exported keying material has $EKM_SIZE bytes, not ${ekm.size}" }

    private fun requireNonce(nonce: ByteArray) =
        require(nonce.size == NONCE_SIZE) { "A nonce has $NONCE_SIZE bytes, not ${nonce.size}" }
}
