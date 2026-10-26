package dev.link2linux.android.core.security

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.SasCode
import dev.link2linux.android.core.security.PairingCrypto.Role
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.cert.CertificateException

/** Every pairing construction against proto/testdata/v1/pairing.txt. */
class PairingCryptoTest {

    private val v = PairingVectors
    private val fpDesktop = Spki.fingerprint(v.bytes("spki_desktop"))
    private val fpPhone = Spki.fingerprint(v.bytes("spki_phone"))
    private val secret = PairingSecret.of(v.bytes("secret"))
    private val ekmPair = v.bytes("ekm_pair")
    private val ekmSas = v.bytes("ekm_sas")

    @Test
    fun fingerprintsMatch() {
        assertEquals(KeyFingerprint.fromHex(v.text("fp_desktop")), fpDesktop)
        assertEquals(KeyFingerprint.fromHex(v.text("fp_phone")), fpPhone)
    }

    @Test
    fun exporterContextIsBothFingerprintsDesktopFirst() {
        val context = PairingCrypto.exporterContext(fpDesktop, fpPhone)
        assertArrayEquals(v.bytes("exporter_context"), context)
        assertEquals(64, context.size)
    }

    @Test
    fun proofsMatch() {
        assertArrayEquals(v.bytes("proof_desktop"), PairingCrypto.proof(secret, Role.DESKTOP, ekmPair))
        assertArrayEquals(v.bytes("proof_phone"), PairingCrypto.proof(secret, Role.PHONE, ekmPair))
    }

    @Test
    fun theDesktopsProofVerifies() {
        assertTrue(PairingCrypto.verifyProof(secret, Role.DESKTOP, ekmPair, v.bytes("proof_desktop")))
    }

    @Test
    fun aProofWithOneFlippedBitIsRejected() {
        val mac = v.bytes("proof_desktop")
        mac[17] = (mac[17].toInt() xor 0x04).toByte()
        assertFalse(PairingCrypto.verifyProof(secret, Role.DESKTOP, ekmPair, mac))
    }

    @Test
    fun aReflectedProofIsRejected() {
        // The phone's own proof sent back as the desktop's.
        assertFalse(PairingCrypto.verifyProof(secret, Role.DESKTOP, ekmPair, v.bytes("proof_phone")))
    }

    @Test
    fun aProofFromAnotherConnectionIsRejected() {
        assertFalse(PairingCrypto.verifyProof(secret, Role.DESKTOP, ekmSas, v.bytes("proof_desktop")))
    }

    @Test
    fun aProofWithTheWrongSecretIsRejected() {
        val other = PairingSecret.of(ByteArray(16))
        assertFalse(PairingCrypto.verifyProof(other, Role.DESKTOP, ekmPair, v.bytes("proof_desktop")))
    }

    @Test
    fun aTruncatedProofIsRejected() {
        assertFalse(PairingCrypto.verifyProof(secret, Role.DESKTOP, ekmPair, v.bytes("proof_desktop").copyOf(31)))
    }

    @Test
    fun sasCommitmentMatches() {
        assertArrayEquals(v.bytes("sas_commitment"), PairingCrypto.sasCommitment(ekmSas, v.bytes("n_p")))
    }

    @Test
    fun sasHmacAndCodeMatch() {
        val nD = v.bytes("n_d")
        val nP = v.bytes("n_p")
        assertArrayEquals(v.bytes("sas_hmac"), PairingCrypto.sasHmac(ekmSas, nD, nP))
        assertEquals(SasCode(v.text("sas_code").toInt()), PairingCrypto.sasCode(ekmSas, nD, nP))
    }

    @Test
    fun sasCodeReadsTheFirstFourBytesBigEndian() {
        // 0x7c834e1f = 2088979999, so the code is 979999 and not a little-endian reading.
        assertEquals("7c834e1f", v.text("sas_hmac").substring(0, 8))
        assertEquals("979 999", PairingCrypto.sasCode(ekmSas, v.bytes("n_d"), v.bytes("n_p")).formatted)
    }

    @Test
    fun secondSasVectorKeepsItsLeadingZero() {
        val nD = v.bytes("n_d_2")
        val nP = v.bytes("n_p")
        assertArrayEquals(v.bytes("sas_hmac_2"), PairingCrypto.sasHmac(ekmSas, nD, nP))
        val code = PairingCrypto.sasCode(ekmSas, nD, nP)
        assertEquals(v.text("sas_code_2"), code.formatted.replace(" ", ""))
        assertEquals("010 917", code.formatted)
    }

    @Test
    fun thePhonesProofVerifiesAndTheTamperedOneDoesNot() {
        assertTrue(PairingCrypto.verifyProof(secret, Role.PHONE, ekmPair, v.bytes("proof_phone")))
        assertFalse(PairingCrypto.verifyProof(secret, Role.PHONE, ekmPair, v.bytes("bad_proof_phone")))
    }

    @Test(expected = IllegalArgumentException::class)
    fun aShortNonceIsRefused() {
        PairingCrypto.sasCommitment(ekmSas, ByteArray(16))
    }

    @Test
    fun aNonCanonicalKeyEncodingIsRefused() {
        val spki = v.bytes("spki_desktop")
        assertTrue(Spki.isCanonicalP256(spki))

        // The same key with a compressed point: valid DER, but not the form both apps hash.
        val y = spki.last().toInt() and 1
        val compressed = hexBytes("3039301306072a8648ce3d020106082a8648ce3d030107032200") +
            byteArrayOf((0x02 + y).toByte()) + spki.copyOfRange(27, 59)
        assertFalse(Spki.isCanonicalP256(compressed))
        assertThrows { Spki.fingerprint(compressed) }
        assertThrows { Spki.fingerprint(spki.copyOf(90)) }
    }

    private fun assertThrows(block: () -> Unit) {
        val error = runCatching(block).exceptionOrNull()
        assertTrue("expected CertificateException, got $error", error is CertificateException)
    }
}
