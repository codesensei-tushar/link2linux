package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.DeviceKind
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket

/** Sessions over loopback TCP; TLS is the transport's job and is not involved here. */
class SessionTest {

    private val phoneHello = hello {
        protocolVersion = 1
        deviceId = "phone-1"
        deviceName = "Phone"
        kind = DeviceKind.DEVICE_KIND_PHONE
        features += "clipboard"
    }
    private val desktopHello = hello {
        protocolVersion = 1
        deviceId = "desk-1"
        deviceName = "Desk"
        kind = DeviceKind.DEVICE_KIND_DESKTOP
    }

    private val sockets = mutableListOf<Socket>()

    private class SocketTransport(private val socket: Socket) : Transport {
        override val input = socket.getInputStream()
        override val output = socket.getOutputStream()
        override fun close() = socket.close()
    }

    /** Returns the phone end and the desktop end of a connected pair. */
    private fun connectedPair(): Pair<Transport, Transport> {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            val client = Socket(InetAddress.getLoopbackAddress(), server.localPort)
            val accepted = server.accept()
            sockets += client
            sockets += accepted
            return SocketTransport(client) to SocketTransport(accepted)
        }
    }

    @After
    fun closeSockets() = sockets.forEach(Socket::close)

    @Test
    fun bothSidesExchangeHello() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val phone = Session(phoneEnd)
        val desktop = Session(desktopEnd)

        withTimeout(5_000) {
            val desktopSaw = async { desktop.handshake(desktopHello) }
            assertEquals(desktopHello, phone.handshake(phoneHello))
            assertEquals(phoneHello, desktopSaw.await())
        }
        assertEquals(SessionState.Established(desktopHello, version = 1), phone.state.value)
    }

    @Test
    fun theHighestVersionInBothRangesIsUsed() {
        fun hello(min: Int, max: Int) = hello { minProtocolVersion = min; protocolVersion = max }
        assertEquals(1, Session.negotiateVersion(hello(0, 1), hello(1, 3)))
        assertEquals(2, Session.negotiateVersion(hello(1, 2), hello(2, 3)))
        assertEquals(3, Session.negotiateVersion(hello(0, 3), hello(0, 3)))
        assertNull(Session.negotiateVersion(hello(0, 1), hello(2, 3)))
        assertNull(Session.negotiateVersion(hello(0, 1), hello(0, 2)))
        assertNull(Session.negotiateVersion(hello(0, 0), hello(0, 0)))
    }

    @Test
    fun aPeerThatAlsoSpeaksANewerVersionIsAccepted() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val newer = desktopHello.toBuilder().setProtocolVersion(2).setMinProtocolVersion(1).build()
        FrameCodec().write(envelope { seq = 1; hello = newer }, desktopEnd.output)
        desktopEnd.output.flush()
        val phone = Session(phoneEnd)

        withTimeout(5_000) { phone.handshake(phoneHello) }
        assertEquals(SessionState.Established(newer, version = 1), phone.state.value)
    }

    @Test
    fun noCommonVersionSendsVersionMismatch() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val newer = desktopHello.toBuilder().setProtocolVersion(3).setMinProtocolVersion(2).build()
        FrameCodec().write(envelope { seq = 1; hello = newer }, desktopEnd.output)
        desktopEnd.output.flush()

        val error = withTimeout(5_000) { runCatching { Session(phoneEnd).handshake(phoneHello) }.exceptionOrNull() }

        assertTrue("got $error", error is VersionMismatchException)
        val desktop = FrameCodec()
        assertTrue(desktop.read(desktopEnd.input)!!.hasHello())
        assertEquals(GoodbyeReason.GOODBYE_REASON_VERSION_MISMATCH, desktop.read(desktopEnd.input)!!.goodbye.reason)
    }

    @Test
    fun aSkippedSequenceNumberIsAProtocolError() = assertProtocolError(
        envelope { seq = 1; hello = desktopHello },
        envelope { seq = 3; ping = ping { nonce = 1 } },
    )

    @Test
    fun aRepeatedSequenceNumberIsAProtocolError() = assertProtocolError(
        envelope { seq = 1; hello = desktopHello },
        envelope { seq = 1; ping = ping { nonce = 1 } },
    )

    @Test
    fun aSecondHelloIsAProtocolError() = assertProtocolError(
        envelope { seq = 1; hello = desktopHello },
        envelope { seq = 2; hello = desktopHello },
    )

    /** The phone reads [fromDesktop] after its Hello, fails, and says why in a Goodbye. */
    private fun assertProtocolError(vararg fromDesktop: Envelope) = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val desktop = FrameCodec()
        fromDesktop.forEach { desktop.write(it, desktopEnd.output) }
        desktopEnd.output.flush()
        val phone = Session(phoneEnd)

        val error = withTimeout(5_000) {
            phone.handshake(phoneHello)
            runCatching { phone.next() }.exceptionOrNull()
        }

        assertTrue("expected SessionException, got $error", error is SessionException)
        assertEquals(SessionState.Closed, phone.state.value)
        val sent = generateSequence { desktop.read(desktopEnd.input) }.toList()
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, sent.last().goodbye.reason)
        assertEquals((1L..sent.size).toList(), sent.map { it.seq })
    }

    @Test
    fun pingIsAnsweredAndFeatureMessagesComeThrough() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val phone = Session(phoneEnd)
        val desktop = FrameCodec()

        withTimeout(5_000) {
            desktop.write(envelope { seq = 1; hello = desktopHello }, desktopEnd.output)
            desktop.write(envelope { seq = 2; ping = ping { nonce = 99 } }, desktopEnd.output)
            // A message with a payload this version does not know yet.
            desktop.write(envelope { seq = 3 }, desktopEnd.output)
            desktopEnd.output.flush()

            phone.handshake(phoneHello)
            val message = phone.incoming().first()

            assertEquals(3L, message.seq)
            assertEquals(1L, desktop.read(desktopEnd.input)?.seq) // the phone's Hello
            val pong = desktop.read(desktopEnd.input)!!
            assertEquals(Envelope.PayloadCase.PONG, pong.payloadCase)
            assertEquals(2L, pong.seq)
            assertEquals(99L, pong.pong.nonce)
        }
    }

    @Test
    fun otherMajorVersionIsRefused() = assertHandshakeFails(
        envelope { seq = 1; hello = desktopHello.toBuilder().setProtocolVersion(2).build() },
    )

    @Test
    fun nonHelloFirstFrameIsRefused() = assertHandshakeFails(
        envelope { seq = 1; ping = ping { nonce = 1 } },
    )

    @Test
    fun peerClosingBeforeHelloIsAnError() = assertHandshakeFails()

    private fun assertHandshakeFails(vararg fromDesktop: Envelope) = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        fromDesktop.forEach { FrameCodec().write(it, desktopEnd.output) }
        desktopEnd.output.flush()
        sockets.last().shutdownOutput() // the desktop end sends nothing more
        val phone = Session(phoneEnd)

        val error = withTimeout(5_000) { runCatching { phone.handshake(phoneHello) }.exceptionOrNull() }

        assertTrue("expected SessionException, got $error", error is SessionException)
        assertEquals(SessionState.Closed, phone.state.value)
    }
}
