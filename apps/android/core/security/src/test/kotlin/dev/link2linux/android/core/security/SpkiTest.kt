package dev.link2linux.android.core.security

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.testing.TestCertificates
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.MessageDigest
import java.security.cert.CertificateException

class SpkiTest {

    @Test
    fun takesTheKeyBytesStraightFromTheCertificate() {
        val identity = TestCertificates.generate()
        val spki = Spki.fromCertificate(identity.certificate.encoded)

        assertEquals(Spki.P256_SIZE, spki.size)
        assertArrayEquals(identity.keyPair.public.encoded, spki)
        assertEquals(
            KeyFingerprint.of(MessageDigest.getInstance("SHA-256").digest(spki)),
            Spki.fingerprint(identity.certificate),
        )
    }

    @Test
    fun identityKeyExposesItsFingerprint() {
        val identity = TestCertificates.generate()
        val key = IdentityKey(identity.privateKey, identity.certificate)
        assertEquals(Spki.fingerprint(identity.certificate), key.fingerprint)
        assertNotEquals(key.fingerprint, IdentityKey(identity.privateKey, TestCertificates.generate().certificate).fingerprint)
    }

    @Test
    fun reissuingTheCertificateKeepsTheFingerprint() {
        val identity = TestCertificates.generate("first name")
        val reissued = TestCertificates.selfSigned(identity.keyPair, "another name")
        assertNotEquals(identity.certificate, reissued)
        assertEquals(Spki.fingerprint(identity.certificate), Spki.fingerprint(reissued))
    }

    @Test
    fun malformedCertificatesAreRefused() {
        val der = TestCertificates.generate().certificate.encoded
        for (broken in listOf(ByteArray(0), der.copyOf(40), byteArrayOf(0x31) + der.copyOfRange(1, der.size))) {
            val error = runCatching { Spki.fromCertificate(broken) }.exceptionOrNull()
            assertTrue("expected CertificateException, got $error", error is CertificateException)
        }
    }
}
