package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.discovery.QrParseResult.Reason
import dev.link2linux.android.core.model.QrPairingOffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress

class QrOfferParserTest {

    private val fp = "_cRaTr6EOO-nZGngbLdW_xKpw0VczVObf2B7Hbdj-bU"
    private val secret = "AAECAwQFBgcICQoLDA0ODw"

    private fun uri(vararg extra: String, f: String = fp, s: String = secret, v: String = "1") =
        "link2linux://pair?v=$v&f=$f&s=$s" + extra.joinToString("") { "&$it" }

    private fun offer(vararg extra: String): QrPairingOffer {
        val result = QrOfferParser.parse(uri("n=Desk", *extra))
        return (result as QrParseResult.Parsed).offer
    }

    private fun invalid(text: String): Reason {
        val result = QrOfferParser.parse(text)
        return (result as? QrParseResult.Invalid)?.reason ?: throw AssertionError("$text parsed as $result")
    }

    private fun addr(literal: String, port: Int) = InetSocketAddress(InetAddress.getByAddress(
        if (':' in literal) {
            val b = java.net.Inet6Address.getByName(literal).address
            b
        } else {
            literal.split('.').map { it.toInt().toByte() }.toByteArray()
        },
    ), port)

    @Test
    fun parsesTheSharedVector() {
        val v = PairingVector.load()
        val offer = (QrOfferParser.parse(v["qr_uri"]) as QrParseResult.Parsed).offer
        assertEquals(v["fp_desktop"], offer.desktopFingerprint.toHex())
        assertEquals(v["secret"], offer.secret.toByteArray().joinToString("") { "%02x".format(it) })
        assertEquals("Büro workstation", offer.desktopName)
        assertEquals(listOf(addr("192.168.1.20", 27381), addr("fd00::20", 27381)), offer.beaconAddresses)
        assertEquals("0C:1A:2B:3C:4D:5E", offer.bluetoothAddress)
    }

    @Test
    fun unknownVersionAsksForAnUpdateEvenWhenTheRestIsUnreadable() {
        assertEquals(QrParseResult.UpdateRequired, QrOfferParser.parse("link2linux://pair?v=2&x=1"))
        assertEquals(QrParseResult.UpdateRequired, QrOfferParser.parse(uri("n=Desk", v = "2")))
        assertEquals(Reason.MALFORMED, invalid("link2linux://pair?f=$fp&n=Desk"))
    }

    @Test
    fun rejectsOtherSchemesFragmentsAndLongInput() {
        assertEquals(Reason.NOT_A_PAIRING_CODE, invalid("https://example.com/?v=1"))
        assertEquals(Reason.NOT_A_PAIRING_CODE, invalid(uri("n=Desk") + "#x"))
        assertEquals(Reason.NOT_A_PAIRING_CODE, invalid(uri("n=" + "a".repeat(1100))))
        assertEquals(Reason.NOT_A_PAIRING_CODE, invalid(uri("n=Büro")))
        assertEquals(Reason.NOT_A_PAIRING_CODE, invalid(uri("n=a b")))
    }

    @Test
    fun rejectsDuplicateParametersAndIgnoresUnknownOnes() {
        assertEquals(Reason.MALFORMED, invalid(uri("n=A", "n=B")))
        assertEquals(Reason.MALFORMED, invalid(uri("n=A", "novalue")))
        assertEquals("Desk", offer("zz=1").desktopName)
    }

    @Test
    fun fingerprintAndSecretMustBeExactAndCanonical() {
        assertEquals(Reason.MISSING_OR_BAD_FINGERPRINT, invalid(uri("n=D", f = fp.dropLast(1))))
        assertEquals(Reason.MISSING_OR_BAD_FINGERPRINT, invalid(uri("n=D", f = fp.dropLast(1) + "V"))) // non-zero trailing bits
        assertEquals(Reason.MISSING_OR_BAD_FINGERPRINT, invalid(uri("n=D", f = "$fp=")))
        assertEquals(Reason.MISSING_OR_BAD_SECRET, invalid(uri("n=D", s = secret + "A")))
        assertEquals(Reason.MISSING_OR_BAD_SECRET, invalid(uri("n=D", s = secret.dropLast(1) + "x")))
        assertEquals(Reason.MISSING_OR_BAD_SECRET, invalid(uri("n=D", s = "AAECAwQFBgcICQoLDA0O+w")))
    }

    @Test
    fun nameIsStrictlyPercentDecoded() {
        assertEquals("a+b c", (QrOfferParser.parse(uri("n=a%2Bb%20c")) as QrParseResult.Parsed).offer.desktopName)
        assertEquals("a+b", (QrOfferParser.parse(uri("n=a+b")) as QrParseResult.Parsed).offer.desktopName)
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=a%2")))
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=a%zz")))
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=%ff%fe")))
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=a%00b")))
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=a%0Ab")))
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=")))
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=" + "a".repeat(65))))
        assertEquals(64, (QrOfferParser.parse(uri("n=" + "a".repeat(64))) as QrParseResult.Parsed).offer.desktopName.length)
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri()))
    }

    @Test
    fun invisibleCharactersAreDroppedFromTheName() {
        fun name(encoded: String) = (QrOfferParser.parse(uri("n=$encoded")) as QrParseResult.Parsed).offer.desktopName
        // Right-to-left override, zero-width space, left-to-right isolate, line separator.
        assertEquals("kseD", name("%E2%80%AEkseD"))
        assertEquals("Desk", name("Desk%E2%80%8B"))
        assertEquals("Desk", name("%E2%81%A6Desk"))
        assertEquals("Desk", name("De%E2%80%A8sk"))
        // Emoji sequences keep their joiner.
        assertEquals("\uD83D\uDC69\u200D\uD83D\uDCBB", name("%F0%9F%91%A9%E2%80%8D%F0%9F%92%BB"))
        // Nothing visible left.
        assertEquals(Reason.MISSING_OR_BAD_NAME, invalid(uri("n=%E2%80%8B%E2%80%AE")))
    }

    @Test
    fun addressesAreLiteralsWithOptionalPorts() {
        assertEquals(emptyList<InetSocketAddress>(), offer("a=").beaconAddresses)
        assertEquals(
            listOf(addr("10.0.0.2", 5000), addr("fd00::1", 6000), addr("fd00::2", 27381)),
            offer("a=10.0.0.2:5000,[fd00::1]:6000,[fd00::2]").beaconAddresses,
        )
        assertTrue(offer("a=fd00::20").beaconAddresses.single().address is java.net.Inet6Address)
    }

    @Test
    fun rejectsBadAddresses() {
        for (a in listOf(
            "example.com", "localhost", "10.0.0", "10.0.0.256", "010.0.0.1", "10.0.0.1:0", "10.0.0.1:70000",
            "10.0.0.1:", "fe80::1", "[fe80::1]:5", "[fd00::1", "[fd00::1]5", "1::2::3", "::", "127.0.0.1",
            "224.0.0.1", "0.0.0.0", "fd00::1%eth0", "1,2,3,4,5", "10.0.0.1,", ",10.0.0.1", "::ffff:1.2.3.4",
        )) {
            assertEquals(a, Reason.BAD_ADDRESSES, invalid(uri("n=D", "a=$a")))
        }
        assertEquals(4, offer("a=10.0.0.1,10.0.0.2,10.0.0.3,10.0.0.4").beaconAddresses.size)
    }

    @Test
    fun bluetoothAddressMustBePublicLookingTwelveHexDigits() {
        assertEquals("0C:BB:CC:DD:EE:00", offer("b=0cbbccddee00").bluetoothAddress)
        assertEquals(null, offer().bluetoothAddress)
        for (b in listOf("0C1A2B3C4D", "0C1A2B3C4D5E6F", "0C1A2B3C4D5G", "0E1A2B3C4D5E", "0D1A2B3C4D5E", "000000000000", "0C:1A:2B:3C:4D:5E")) {
            assertEquals(b, Reason.BAD_BLUETOOTH_ADDRESS, invalid(uri("n=D", "b=$b")))
        }
    }
}
