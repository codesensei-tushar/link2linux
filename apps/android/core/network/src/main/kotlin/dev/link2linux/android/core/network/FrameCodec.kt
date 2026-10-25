package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.Envelope
import java.io.DataInputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * Standard protobuf length-delimited framing: a varint length prefix, then
 * one serialized [Envelope]. These are the bytes of `writeDelimitedTo` /
 * `parseDelimitedFrom`, and of the desktop codec (prost's
 * `encode_length_delimited`).
 *
 * Frames whose body is longer than [maxFrameBytes] are refused in both
 * directions; when reading, before any of the body is buffered.
 */
class FrameCodec(val maxFrameBytes: Int = DEFAULT_MAX_FRAME_BYTES) {

    init {
        require(maxFrameBytes > 0) { "maxFrameBytes must be positive" }
    }

    /** Writes one frame. The caller flushes. */
    fun write(envelope: Envelope, output: OutputStream) {
        val size = envelope.serializedSize
        if (size > maxFrameBytes) throw FrameTooLargeException(size.toLong(), maxFrameBytes)
        envelope.writeDelimitedTo(output)
    }

    /**
     * Reads one frame. Returns null when the stream ends cleanly between
     * frames, and throws [EOFException] when it ends inside one.
     */
    fun read(input: InputStream): Envelope? {
        val length = readLength(input) ?: return null
        if (length > maxFrameBytes) throw FrameTooLargeException(length, maxFrameBytes)
        val body = ByteArray(length.toInt())
        DataInputStream(input).readFully(body)
        return Envelope.parseFrom(body)
    }

    /** Decodes the varint length prefix, or returns null at end of stream. */
    private fun readLength(input: InputStream): Long? {
        var value = 0L
        var shift = 0
        while (shift < 64) {
            val byte = input.read()
            if (byte == -1) {
                if (shift == 0) return null
                throw EOFException("Stream ended inside a frame length")
            }
            value = value or ((byte and 0x7f).toLong() shl shift)
            if (byte and 0x80 == 0) return value
            shift += 7
        }
        throw IOException("Frame length varint is longer than 10 bytes")
    }

    companion object {
        /** Largest frame body, in bytes. The desktop codec uses the same default. */
        const val DEFAULT_MAX_FRAME_BYTES: Int = 4 * 1024 * 1024
    }
}

class FrameTooLargeException(val length: Long, val limit: Int) :
    IOException("Frame body of $length bytes is over the $limit-byte limit")
