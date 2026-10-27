package dev.link2linux.android.core.screen

import java.io.ByteArrayOutputStream

/**
 * Turns an H.264 encoder's output into what `ScreenFrame` and
 * `ScreenFormat` carry (docs/protocol.md, "Video"). Encoders write NAL
 * units behind start codes (Annex B); the protocol puts a 4-byte big-endian
 * length in front of each instead, and describes the stream with an avcC
 * record (ISO/IEC 14496-15, AVCDecoderConfigurationRecord).
 */
object AvcStream {

    private const val NAL_SPS = 7
    private const val NAL_PPS = 8

    /** The most `ScreenFormat.codec_config` may hold. */
    const val MAX_CONFIG_BYTES = 4 * 1024

    /**
     * The NAL units of [annexB], without their start codes, as ranges of
     * it. Bytes before the first start code are not a NAL unit and are
     * skipped, and so are empty units.
     */
    fun nalUnits(annexB: ByteArray): List<IntRange> {
        val starts = ArrayList<Pair<Int, Int>>() // where each start code begins, and where its unit begins
        var i = 0
        while (i + 2 < annexB.size) {
            if (annexB[i].toInt() == 0 && annexB[i + 1].toInt() == 0 && annexB[i + 2].toInt() == 1) {
                // A zero before 00 00 01 belongs to a 4-byte start code, not to the unit before it.
                val codeStart = if (i > 0 && annexB[i - 1].toInt() == 0) i - 1 else i
                starts += codeStart to i + 3
                i += 3
            } else {
                i++
            }
        }
        return starts.mapIndexedNotNull { n, (_, from) ->
            val end = if (n + 1 < starts.size) starts[n + 1].first else annexB.size
            (from until end).takeIf { !it.isEmpty() }
        }
    }

    /** [annexB] with a 4-byte big-endian length in front of each NAL unit instead of its start code. */
    fun lengthPrefixed(annexB: ByteArray): ByteArray {
        val units = nalUnits(annexB)
        val out = ByteArray(units.sumOf { 4 + it.count() })
        var at = 0
        for (unit in units) {
            val length = unit.count()
            out[at] = (length ushr 24).toByte()
            out[at + 1] = (length ushr 16).toByte()
            out[at + 2] = (length ushr 8).toByte()
            out[at + 3] = length.toByte()
            annexB.copyInto(out, at + 4, unit.first, unit.last + 1)
            at += 4 + length
        }
        return out
    }

    /**
     * The avcC record for the parameter sets in [parts], the encoder's
     * codec-specific data (on Android csd-0 and csd-1) in Annex B. Null
     * when they hold no sequence or no picture parameter set, or the record
     * would be larger than [MAX_CONFIG_BYTES].
     */
    fun decoderConfig(vararg parts: ByteArray): ByteArray? {
        val sps = ArrayList<ByteArray>()
        val pps = ArrayList<ByteArray>()
        for (part in parts) {
            for (unit in nalUnits(part)) {
                val bytes = part.copyOfRange(unit.first, unit.last + 1)
                when (bytes[0].toInt() and 0x1F) {
                    NAL_SPS -> sps += bytes
                    NAL_PPS -> pps += bytes
                }
            }
        }
        // The record counts sequence parameter sets in 5 bits, and an SPS needs its profile and level bytes.
        if (sps.isEmpty() || pps.isEmpty() || sps.size > 31 || pps.size > 255 || sps[0].size < 4) return null
        val out = ByteArrayOutputStream()
        out.write(1) // configurationVersion
        out.write(sps[0][1].toInt()) // AVCProfileIndication
        out.write(sps[0][2].toInt()) // profile_compatibility
        out.write(sps[0][3].toInt()) // AVCLevelIndication
        out.write(0xFC or 3) // reserved bits, then lengthSizeMinusOne: 4-byte lengths
        out.write(0xE0 or sps.size) // reserved bits, then numOfSequenceParameterSets
        for (set in sps) out.writeSet(set)
        out.write(pps.size)
        for (set in pps) out.writeSet(set)
        return out.toByteArray().takeIf { it.size <= MAX_CONFIG_BYTES }
    }

    private fun ByteArrayOutputStream.writeSet(set: ByteArray) {
        write(set.size ushr 8)
        write(set.size and 0xFF)
        write(set)
    }
}
