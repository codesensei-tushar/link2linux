package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PresenceKey
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TagsAndBeaconTest {

    private fun ByteArray.hex() = joinToString("") { "%02x".format(it) }

    @Test
    fun presenceTagMatchesTheSharedVector() {
        val v = PairingVector.load()
        val tag = Tags.presence(PresenceKey.of(v.bytes("presence_key")), v.bytes("presence_nonce"))
        assertEquals(v["txt_t"], tag.toBase64Url())
        assertEquals(v["presence_tag"], tag.hex())
    }

    @Test
    fun pairingTagMatchesTheSharedVector() {
        val v = PairingVector.load()
        val tag = Tags.pairing(PairingSecret.of(v.bytes("secret")))
        assertEquals(v["txt_q"], tag.toBase64Url())
        assertEquals(v["pairing_tag"], tag.hex())
    }

    @Test
    fun nonceEncodesAsInTheSharedVector() {
        val v = PairingVector.load()
        assertEquals(v["txt_n"], v.bytes("presence_nonce").toBase64Url())
    }

    @Test
    fun beaconsMatchTheSharedVector() {
        val v = PairingVector.load()
        val time = v["beacon_time"].toLong()
        val port = v["beacon_port"].toInt()
        val nonce = v.bytes("beacon_nonce")
        val presence = Beacon.presence(PresenceKey.of(v.bytes("presence_key")), port, time, nonce)
        val pairing = Beacon.pairing(PairingSecret.of(v.bytes("secret")), port, time, nonce)
        assertEquals(Beacon.SIZE, presence.size)
        assertArrayEquals(v.bytes("beacon"), presence)
        assertArrayEquals(v.bytes("beacon_pairing"), pairing)
    }

    @Test
    fun txtCarriesOneTagPerDesktopUpToThree() {
        val keys = List(5) { i -> PresenceKey.of(ByteArray(32) { i.toByte() }) }
        val nonce = ByteArray(8) { it.toByte() }
        val state = AnnounceState(keys, PairingSecret.of(ByteArray(16)), sasMode = true)
        val txt = Txt.build(state, nonce)
        assertEquals(setOf("v", "n", "t", "t2", "t3", "q", "pm"), txt.keys)
        assertEquals("1", txt["v"])
        assertEquals(Tags.presence(keys[1], nonce).toBase64Url(), txt["t2"])
        assertTrue(Txt.sizeInBytes(txt) < 200)
    }

    @Test
    fun unpairedPhoneAnnouncesOnlyTheVersion() {
        assertEquals(mapOf("v" to "1"), Txt.build(AnnounceState(), ByteArray(8)))
        assertEquals(mapOf("v" to "1", "pm" to "1"), Txt.build(AnnounceState(sasMode = true), ByteArray(8)))
    }

    @Test
    fun instanceNameIsPrefixAndEightHexDigits() {
        assertEquals("l2l-0a1bffc2", Txt.instanceName(byteArrayOf(0x0a, 0x1b, 0xff.toByte(), 0xc2.toByte())))
    }
}
