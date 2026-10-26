package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.Envelope
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
 * Frames whose body is longer than [maxFrameBytes], or longer than
 * [MAX_MESSAGE_FRAME_BYTES] without an image, are refused in both
 * directions; when reading, the length is checked before any of the body is
 * buffered, and the body is checked with [FrameLimits] before it is parsed.
 */
class FrameCodec(val maxFrameBytes: Int = DEFAULT_MAX_FRAME_BYTES) {

    init {
        require(maxFrameBytes > 0) { "maxFrameBytes must be positive" }
    }

    /** The largest frame body without an image. */
    private val maxMessageBytes = minOf(maxFrameBytes, MAX_MESSAGE_FRAME_BYTES)

    /** Writes one frame. The caller flushes. */
    fun write(envelope: Envelope, output: OutputStream) {
        checkSize(envelope)
        envelope.writeDelimitedTo(output)
    }

    /** Throws [FrameTooLargeException] when [envelope] is too large for one frame. */
    fun checkSize(envelope: Envelope) {
        val size = envelope.serializedSize
        val limit = if (envelope.hasImageData()) maxFrameBytes else maxMessageBytes
        if (size > limit) throw FrameTooLargeException(size.toLong(), limit)
    }

    /**
     * Reads one frame. Returns null when the stream ends cleanly between
     * frames, and throws [EOFException] when it ends inside one.
     */
    fun read(input: InputStream): Envelope? {
        val length = readLength(input) ?: return null
        // A length with the 64th bit set reads as negative.
        if (length < 0 || length > maxFrameBytes) throw FrameTooLargeException(length, maxFrameBytes)
        val body = readBody(input, length.toInt())
        FrameLimits.check(body, maxMessageBytes)
        return Envelope.parseFrom(body)
    }

    /** Decodes the varint length prefix, or returns null at end of stream. */
    private fun readLength(input: InputStream): Long? {
        var value = 0L
        for (index in 0 until MAX_VARINT_BYTES) {
            val byte = input.read()
            if (byte == -1) {
                if (index == 0) return null
                throw EOFException("Stream ended inside a frame length")
            }
            // The tenth byte may only hold the 64th bit.
            if (index == MAX_VARINT_BYTES - 1 && byte > 1) throw IOException("Frame length varint overflows 64 bits")
            value = value or ((byte and 0x7f).toLong() shl (7 * index))
            if (byte and 0x80 == 0) return value
        }
        throw IOException("Frame length varint is longer than 10 bytes")
    }

    /**
     * Reads a body of [length] bytes into a buffer that grows as the bytes
     * arrive, so that a prefix alone never makes the codec allocate the
     * whole length.
     */
    private fun readBody(input: InputStream, length: Int): ByteArray {
        var body = ByteArray(minOf(length, INITIAL_BODY_BYTES))
        var filled = 0
        while (filled < length) {
            if (filled == body.size) body = body.copyOf(minOf(length, body.size * 2))
            val read = input.read(body, filled, body.size - filled)
            if (read == -1) throw EOFException("Stream ended inside a frame")
            filled += read
        }
        return body
    }

    companion object {
        /**
         * Largest frame body, in bytes: room for an `ImageData` with an
         * image of 1 MiB, the largest the protocol allows. The desktop codec
         * uses the same limits.
         */
        const val DEFAULT_MAX_FRAME_BYTES: Int = 1280 * 1024

        /** Largest frame body without an image. Senders keep such frames below 256 KiB. */
        const val MAX_MESSAGE_FRAME_BYTES: Int = 512 * 1024

        /**
         * Largest frame body until the session starts: during the Hello
         * exchange and the pairing exchange, whose messages take well under
         * 1 KiB, and before the peer is known to be paired.
         */
        const val HANDSHAKE_MAX_FRAME_BYTES: Int = 16 * 1024

        private const val MAX_VARINT_BYTES = 10
        private const val INITIAL_BODY_BYTES = 64 * 1024
    }
}

/** A frame breaks a limit of docs/protocol.md, "Framing". */
open class FrameLimitException(message: String) : IOException(message)

class FrameTooLargeException(val length: Long, val limit: Int) :
    FrameLimitException("Frame body of ${java.lang.Long.toUnsignedString(length)} bytes is over the $limit-byte limit")

/** A frame's repeated field holds more entries than the protocol allows. */
class TooManyEntriesException(val field: String, val limit: Int) :
    FrameLimitException("Frame holds more than $limit entries of $field")
