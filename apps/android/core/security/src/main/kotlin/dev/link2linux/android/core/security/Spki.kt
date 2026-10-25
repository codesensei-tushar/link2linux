package dev.link2linux.android.core.security

import dev.link2linux.android.core.model.KeyFingerprint
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate

/**
 * Key fingerprints, taken over the SubjectPublicKeyInfo exactly as it
 * appears in a certificate. Only the canonical P-256 encoding is accepted:
 * a named curve and an uncompressed point, 91 bytes. Both apps check this,
 * so they always hash the same bytes.
 */
object Spki {

    /** The 91-byte P-256 SubjectPublicKeyInfo starts with these 27 bytes, then X and Y. */
    private val P256_PREFIX = byteArrayOf(
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86.toByte(), 0x48, 0xce.toByte(), 0x3d, 0x02, 0x01,
        0x06, 0x08, 0x2a, 0x86.toByte(), 0x48, 0xce.toByte(), 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        0x04,
    )

    /** Length of the canonical P-256 SubjectPublicKeyInfo. */
    const val P256_SIZE: Int = 91

    /** Whether [spki] is the canonical P-256 encoding. */
    fun isCanonicalP256(spki: ByteArray): Boolean =
        spki.size == P256_SIZE && spki.copyOfRange(0, P256_PREFIX.size).contentEquals(P256_PREFIX)

    /** `fp` of a SubjectPublicKeyInfo, which must be canonical P-256. */
    fun fingerprint(spki: ByteArray): KeyFingerprint {
        if (!isCanonicalP256(spki)) throw CertificateException("Not a P-256 key in its canonical encoding")
        return KeyFingerprint.of(MessageDigest.getInstance("SHA-256").digest(spki))
    }

    /** `fp` of a certificate's key, from the bytes in the certificate. */
    fun fingerprint(certificate: X509Certificate): KeyFingerprint = fingerprint(fromCertificate(certificate.encoded))

    /**
     * The DER SubjectPublicKeyInfo inside a DER certificate, byte for byte.
     *
     * @throws CertificateException if the certificate is malformed.
     */
    fun fromCertificate(der: ByteArray): ByteArray {
        val reader = DerReader(der)
        val certificate = reader.element(0, der.size, SEQUENCE)
        val tbs = reader.element(certificate.contentStart, certificate.end, SEQUENCE)
        var element = reader.element(tbs.contentStart, tbs.end)
        if (element.tag == VERSION) element = reader.element(element.end, tbs.end)
        // serialNumber, signature, issuer, validity, subject, then the key.
        repeat(5) { element = reader.element(element.end, tbs.end) }
        if (element.tag != SEQUENCE) throw CertificateException("No SubjectPublicKeyInfo in the certificate")
        return der.copyOfRange(element.start, element.end)
    }

    private const val SEQUENCE = 0x30
    private const val VERSION = 0xa0
}

/** Just enough DER to walk the top of a certificate. */
private class DerReader(private val bytes: ByteArray) {

    class Element(val tag: Int, val start: Int, val contentStart: Int, val end: Int)

    fun element(at: Int, limit: Int, expectedTag: Int? = null): Element {
        if (at + 2 > limit) throw CertificateException("Truncated DER")
        val tag = bytes[at].toInt() and 0xff
        if (expectedTag != null && tag != expectedTag) throw CertificateException("Unexpected DER tag $tag")
        var position = at + 1
        val first = bytes[position++].toInt() and 0xff
        val length = when {
            first < 0x80 -> first
            first == 0x80 || first > 0x83 -> throw CertificateException("Unsupported DER length")
            else -> {
                var value = 0
                repeat(first and 0x7f) {
                    if (position >= limit) throw CertificateException("Truncated DER")
                    value = (value shl 8) or (bytes[position++].toInt() and 0xff)
                }
                value
            }
        }
        val end = position + length
        if (length < 0 || end > limit) throw CertificateException("DER element runs past its parent")
        return Element(tag, at, position, end)
    }
}
