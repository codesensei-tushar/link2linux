package dev.link2linux.android.core.network

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.PhoneNotification
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.imageData
import dev.link2linux.protocol.v1.notificationSyncBatch
import dev.link2linux.protocol.v1.ping
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream

class FrameCodecTest {

    private val codec = FrameCodec()

    private fun encode(vararg envelopes: Envelope, codec: FrameCodec = this.codec): ByteArray =
        ByteArrayOutputStream().also { out -> envelopes.forEach { codec.write(it, out) } }.toByteArray()

    private fun decodeAll(bytes: ByteArray, codec: FrameCodec = this.codec): List<Envelope> {
        val input = ByteArrayInputStream(bytes)
        return generateSequence { codec.read(input) }.toList()
    }

    @Test
    fun pingAndPongFramesMatchTheSharedBytes() {
        val (_, ping, pong) = SharedVectors.envelopes
        assertArrayEquals(hex("0b 08 02 5a 07 08 cb 89 ec 8f f7 23"), encode(ping))
        assertArrayEquals(hex("0b 08 03 62 07 08 cb 89 ec 8f f7 23"), encode(pong))
    }

    @Test
    fun longFrameUsesTwoByteLengthPrefix() {
        val hello = SharedVectors.envelopes.first()
        val bytes = encode(hello)
        assertEquals(131, hello.serializedSize)
        assertArrayEquals(hex("83 01"), bytes.copyOfRange(0, 2))
        assertEquals(listOf(hello), decodeAll(bytes))
    }

    @Test
    fun sharedFramesFileDecodesAndReencodesExactly() {
        val file = SharedVectors.framesFile()
        assumeTrue("proto/testdata/v1/frames.bin is not in this checkout", file != null)
        val bytes = file!!.readBytes()

        assertEquals(SharedVectors.envelopes, decodeAll(bytes))
        assertArrayEquals(bytes, encode(*SharedVectors.envelopes.toTypedArray()))
    }

    @Test
    fun sameBytesAsProtobufDelimitedApi() {
        val envelopes = SharedVectors.envelopes
        val viaProtobuf = ByteArrayOutputStream().also { out -> envelopes.forEach { it.writeDelimitedTo(out) } }
        assertArrayEquals(viaProtobuf.toByteArray(), encode(*envelopes.toTypedArray()))

        val input = ByteArrayInputStream(encode(*envelopes.toTypedArray()))
        assertEquals(envelopes, generateSequence { Envelope.parseDelimitedFrom(input) }.toList())
    }

    @Test
    fun emptyStreamHasNoFrames() {
        assertNull(codec.read(ByteArrayInputStream(ByteArray(0))))
    }

    @Test(expected = EOFException::class)
    fun streamEndingInsideFrameIsAnError() {
        val bytes = encode(SharedVectors.envelopes[1])
        codec.read(ByteArrayInputStream(bytes.copyOf(bytes.size - 1)))
    }

    @Test(expected = EOFException::class)
    fun streamEndingInsideLengthIsAnError() {
        codec.read(ByteArrayInputStream(hex("83")))
    }

    @Test
    fun oversizedIncomingFrameIsRefusedBeforeReadingBody() {
        val small = FrameCodec(maxFrameBytes = 10)
        // Prefix announces 11 bytes; none of the body follows.
        val error = runCatching { small.read(ByteArrayInputStream(hex("0b"))) }.exceptionOrNull()
        assertEquals(FrameTooLargeException::class, error?.let { it::class })
        assertEquals(11L, (error as FrameTooLargeException).length)
    }

    @Test
    fun hugeLengthPrefixIsRefused() {
        // 2^35: would overflow a 32-bit length.
        val error = runCatching { codec.read(ByteArrayInputStream(hex("80 80 80 80 80 01"))) }.exceptionOrNull()
        assertEquals(1L shl 35, (error as FrameTooLargeException).length)
    }

    @Test(expected = FrameTooLargeException::class)
    fun oversizedOutgoingFrameIsRefused() {
        val envelope = envelope { seq = 1; ping = ping { nonce = 1 } }
        FrameCodec(maxFrameBytes = envelope.serializedSize - 1).write(envelope, ByteArrayOutputStream())
    }

    @Test
    fun frameAtTheLimitIsAccepted() {
        val envelope = SharedVectors.envelopes[1]
        val exact = FrameCodec(maxFrameBytes = envelope.serializedSize)
        assertEquals(listOf(envelope), decodeAll(encode(envelope, codec = exact), codec = exact))
    }

    @Test
    fun aLengthWithTheTopBitSetIsTooLarge() {
        // 2^64 - 1 and 2^63 + 16: negative as a Long.
        for (prefix in listOf("ff ff ff ff ff ff ff ff ff 01", "90 80 80 80 80 80 80 80 80 01")) {
            val error = runCatching { codec.read(ByteArrayInputStream(hex(prefix))) }.exceptionOrNull()
            assertTrue("$prefix: got $error", error is FrameTooLargeException)
        }
    }

    @Test
    fun aLengthOverflowing64BitsIsRefused() {
        val error = runCatching { codec.read(ByteArrayInputStream(hex("ff ff ff ff ff ff ff ff ff 02"))) }.exceptionOrNull()
        assertEquals(IOException::class, error?.let { it::class })
    }

    @Test(expected = EOFException::class)
    fun aPrefixAloneIsNotABody() {
        // Announces 1 MiB, then the stream ends.
        codec.read(ByteArrayInputStream(hex("80 80 40 08 01")))
    }

    @Test
    fun aBodyArrivingInPiecesIsReadWhole() {
        val big = envelope {
            seq = 1
            imageData = imageData { data = ByteString.copyFrom(ByteArray(300_000) { it.toByte() }) }
        }
        val trickle = object : InputStream() {
            val bytes = ByteArrayInputStream(encode(big))
            override fun read() = bytes.read()
            override fun read(b: ByteArray, off: Int, len: Int) = bytes.read(b, off, minOf(len, 777))
        }
        assertEquals(big, codec.read(trickle))
    }

    @Test
    fun onlyAnImageMayExceedTheMessageLimit() {
        val image = envelope {
            seq = 1
            imageData = imageData { data = ByteString.copyFrom(ByteArray(1024 * 1024)) }
        }
        assertEquals(listOf(image), decodeAll(encode(image)))

        val batch = envelope {
            seq = 1
            notificationSyncBatch = notificationSyncBatch { removedKeys += "k".repeat(FrameCodec.MAX_MESSAGE_FRAME_BYTES) }
        }
        val written = runCatching { encode(batch) }.exceptionOrNull()
        assertEquals(FrameCodec.MAX_MESSAGE_FRAME_BYTES, (written as FrameTooLargeException).limit)
        val bytes = ByteArrayOutputStream().also { batch.writeDelimitedTo(it) }.toByteArray()
        val read = runCatching { decodeAll(bytes) }.exceptionOrNull()
        assertEquals(FrameCodec.MAX_MESSAGE_FRAME_BYTES, (read as FrameTooLargeException).limit)
    }

    @Test
    fun repeatedFieldsAreCountedBeforeParsing() {
        fun batch(count: Int) = envelope {
            seq = 1
            notificationSyncBatch = notificationSyncBatch { notifications += List(count) { PhoneNotification.getDefaultInstance() } }
        }
        assertEquals(listOf(batch(100)), decodeAll(encode(batch(100))))
        // 600,000 empty notifications: within the frame limit, and around
        // 150 MiB once parsed.
        val bomb = ByteArrayOutputStream().also { batch(600_000).writeDelimitedTo(it) }.toByteArray()
        val error = runCatching { decodeAll(bomb) }.exceptionOrNull()
        assertEquals("PhoneNotification", (error as TooManyEntriesException).field)
    }
}
