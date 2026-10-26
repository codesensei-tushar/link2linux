package dev.link2linux.android.core.testing

import java.io.ByteArrayOutputStream
import java.math.BigInteger
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.Signature
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec

/** A software P-256 key and its self-signed certificate. */
class TestIdentity(val keyPair: KeyPair, val certificate: X509Certificate) {
    val privateKey: PrivateKey get() = keyPair.private
}

/**
 * Self-signed X.509 v3 certificates for P-256 keys, built with the JDK alone.
 * The phone gets its certificate from the Android Keystore and the desktop
 * from its own TLS library, so this exists only to give JVM tests keys of
 * the same shape: ECDSA P-256 with SHA-256, named curve, uncompressed point.
 */
object TestCertificates {

    private val random = SecureRandom()

    /** A new P-256 key with a self-signed certificate for [commonName]. */
    fun generate(commonName: String = "link2linux test"): TestIdentity {
        val keyPair = KeyPairGenerator.getInstance("EC").run {
            initialize(ECGenParameterSpec("secp256r1"), random)
            generateKeyPair()
        }
        return TestIdentity(keyPair, selfSigned(keyPair, commonName))
    }

    /** A self-signed certificate for [keyPair], valid from 1970 to 2049. */
    fun selfSigned(keyPair: KeyPair, commonName: String): X509Certificate {
        val name = sequence(set(sequence(COMMON_NAME_OID, tlv(UTF8_STRING, commonName.toByteArray()))))
        val tbs = sequence(
            tlv(0xa0, tlv(INTEGER, byteArrayOf(2))), // v3
            tlv(INTEGER, BigInteger(63, random).setBit(62).toByteArray()),
            ECDSA_WITH_SHA256,
            name,
            sequence(tlv(UTC_TIME, "700101000000Z".toByteArray()), tlv(UTC_TIME, "491231235959Z".toByteArray())),
            name,
            keyPair.public.encoded,
        )
        val signature = Signature.getInstance("SHA256withECDSA").run {
            initSign(keyPair.private)
            update(tbs)
            sign()
        }
        val der = sequence(tbs, ECDSA_WITH_SHA256, tlv(BIT_STRING, byteArrayOf(0) + signature))
        return CertificateFactory.getInstance("X.509").generateCertificate(der.inputStream()) as X509Certificate
    }

    private const val INTEGER = 0x02
    private const val BIT_STRING = 0x03
    private const val OBJECT_ID = 0x06
    private const val UTF8_STRING = 0x0c
    private const val UTC_TIME = 0x17

    private val ECDSA_WITH_SHA256 = sequence(tlv(OBJECT_ID, bytes(0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02)))
    private val COMMON_NAME_OID = tlv(OBJECT_ID, bytes(0x55, 0x04, 0x03))

    private fun bytes(vararg values: Int) = ByteArray(values.size) { values[it].toByte() }

    private fun sequence(vararg parts: ByteArray) = tlv(0x30, *parts)

    private fun set(vararg parts: ByteArray) = tlv(0x31, *parts)

    private fun tlv(tag: Int, vararg parts: ByteArray): ByteArray {
        val content = ByteArrayOutputStream().apply { parts.forEach(::write) }.toByteArray()
        val length = content.size
        val header = when {
            length < 0x80 -> bytes(tag, length)
            length < 0x100 -> bytes(tag, 0x81, length)
            else -> bytes(tag, 0x82, length shr 8, length and 0xff)
        }
        return header + content
    }
}
