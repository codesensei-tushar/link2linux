package dev.link2linux.android.core.screen

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AvcStreamTest {

    private fun bytes(vararg values: Int) = ByteArray(values.size) { values[it].toByte() }

    // A baseline SPS (profile 66, level 31) and a PPS, cut short; only their first bytes matter here.
    private val sps = bytes(0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01)
    private val pps = bytes(0x68, 0xCE, 0x3C, 0x80)

    @Test
    fun nalUnitsAreFoundBehindThreeAndFourByteStartCodes() {
        val stream = bytes(0, 0, 0, 1, 0x65, 1, 2, 0, 0, 1, 0x41, 3)

        val units = AvcStream.nalUnits(stream).map { stream.copyOfRange(it.first, it.last + 1) }

        assertEquals(2, units.size)
        assertArrayEquals(bytes(0x65, 1, 2), units[0])
        assertArrayEquals(bytes(0x41, 3), units[1])
    }

    @Test
    fun bytesBeforeTheFirstStartCodeAndEmptyUnitsAreSkipped() {
        val stream = bytes(9, 9, 0, 0, 1, 0, 0, 1, 0x41)

        assertEquals(listOf(8..8), AvcStream.nalUnits(stream))
    }

    @Test
    fun eachUnitGetsAFourByteBigEndianLength() {
        val unit = ByteArray(300) { 0x41 }
        val stream = bytes(0, 0, 0, 1) + unit + bytes(0, 0, 1, 0x65, 7)

        val framed = AvcStream.lengthPrefixed(stream)

        assertArrayEquals(bytes(0, 0, 1, 44) + unit + bytes(0, 0, 0, 2, 0x65, 7), framed)
    }

    @Test
    fun theDecoderConfigCarriesTheParameterSetsWithTheProfileOfTheSps() {
        val config = AvcStream.decoderConfig(bytes(0, 0, 0, 1) + sps, bytes(0, 0, 0, 1) + pps)

        val expected = bytes(1, 0x42, 0xC0, 0x1F, 0xFF, 0xE1, 0, sps.size) + sps + bytes(1, 0, pps.size) + pps
        assertArrayEquals(expected, config)
    }

    @Test
    fun parameterSetsMayShareOnePart() {
        val both = bytes(0, 0, 0, 1) + sps + bytes(0, 0, 0, 1) + pps

        assertArrayEquals(AvcStream.decoderConfig(bytes(0, 0, 1) + sps, bytes(0, 0, 1) + pps), AvcStream.decoderConfig(both))
    }

    @Test
    fun withoutBothParameterSetsThereIsNoConfig() {
        assertNull(AvcStream.decoderConfig(bytes(0, 0, 0, 1) + sps))
        assertNull(AvcStream.decoderConfig(bytes(0, 0, 0, 1) + pps))
        assertNull(AvcStream.decoderConfig(ByteArray(0)))
    }

    @Test
    fun aConfigLargerThanTheProtocolAllowsIsRefused() {
        val hugeSps = sps + ByteArray(AvcStream.MAX_CONFIG_BYTES)

        assertNull(AvcStream.decoderConfig(bytes(0, 0, 0, 1) + hugeSps, bytes(0, 0, 0, 1) + pps))
    }
}
