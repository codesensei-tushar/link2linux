package dev.link2linux.android.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SecretsAndFingerprintTest {

    // fp_desktop from proto/testdata/v1/pairing.txt, and its QR form from qr_uri there.
    private val hex = "fdc45a4ebe8438efa76469e06cb756ff12a9c3455ccd539b7f607b1db763f9b5"
    private val base64Url = "_cRaTr6EOO-nZGngbLdW_xKpw0VczVObf2B7Hbdj-bU"

    @Test
    fun fingerprintRoundTripsThroughHexAndBase64Url() {
        val fromHex = KeyFingerprint.fromHex(hex)
        val fromQr = KeyFingerprint.fromBase64Url(base64Url)

        assertEquals(fromHex, fromQr)
        assertEquals(fromHex.hashCode(), fromQr.hashCode())
        assertEquals(hex, fromQr.toHex())
        assertEquals(base64Url, fromHex.toBase64Url())
        assertEquals(hex, KeyFingerprint.fromHex(hex.uppercase()).toHex())
    }

    @Test
    fun fingerprintKeepsItsOwnCopy() {
        val bytes = ByteArray(32)
        val fingerprint = KeyFingerprint.of(bytes)
        bytes[0] = 1
        fingerprint.toByteArray()[1] = 1
        assertEquals(KeyFingerprint.of(ByteArray(32)), fingerprint)
    }

    @Test(expected = IllegalArgumentException::class)
    fun fingerprintMustHave32Bytes() {
        KeyFingerprint.of(ByteArray(31))
    }

    @Test(expected = IllegalArgumentException::class)
    fun paddedBase64IsRefused() {
        KeyFingerprint.fromBase64Url("$base64Url=")
    }

    @Test
    fun secretsCompareByContentAndNeverPrintTheirBytes() {
        val secret = PairingSecret.of(ByteArray(16) { it.toByte() })
        assertEquals(PairingSecret.of(ByteArray(16) { it.toByte() }), secret)
        assertNotEquals(PairingSecret.of(ByteArray(16)), secret)
        assertEquals("PairingSecret(redacted)", secret.toString())

        val presenceKey = PresenceKey.of(ByteArray(32) { 7 })
        assertEquals("PresenceKey(redacted)", presenceKey.toString())
        assertFalse("07" in presenceKey.toString())
    }

    @Test(expected = IllegalArgumentException::class)
    fun secretMustHave16Bytes() {
        PairingSecret.of(ByteArray(32))
    }

    @Test
    fun recordAcceptsItsCurrentAndNextKey() {
        val current = KeyFingerprint.of(ByteArray(32) { 1 })
        val next = KeyFingerprint.of(ByteArray(32) { 2 })
        val record = PairedDevice(
            device = Device(DeviceId("desk"), "Desk", DeviceKind.DESKTOP),
            fingerprint = current,
            pairedAtEpochMillis = 0,
            presenceKey = PresenceKey.of(ByteArray(32)),
            nextFingerprint = next,
        )
        assertTrue(record.accepts(current))
        assertTrue(record.accepts(next))
        assertFalse(record.accepts(KeyFingerprint.of(ByteArray(32))))
    }
}
