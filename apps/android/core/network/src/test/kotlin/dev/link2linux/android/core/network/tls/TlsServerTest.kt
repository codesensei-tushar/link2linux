package dev.link2linux.android.core.network.tls

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.security.IdentityKey
import dev.link2linux.android.core.security.IdentityKeySource
import dev.link2linux.android.core.security.PairingCrypto
import dev.link2linux.android.core.security.Spki
import dev.link2linux.android.core.testing.TestCertificates
import org.conscrypt.Conscrypt
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.BindException
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.interfaces.ECPrivateKey
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import javax.net.ssl.SSLSocket

/**
 * The phone's TLS server against a desktop client over loopback, both on
 * Conscrypt 2.8.0 (its OpenJDK build here).
 */
class TlsServerTest {

    private val phone = TestCertificates.generate("test phone")
    private val phoneKey = Spki.fingerprint(phone.certificate)
    private val executor = Executors.newCachedThreadPool()
    private val listeners = mutableListOf<ServerSocket>()

    @After
    fun tearDown() {
        listeners.forEach(ServerSocket::close)
        executor.shutdownNow()
    }

    private fun server(identity: IdentityKeySource = phone.asSource(), accepts: (KeyFingerprint) -> Boolean) =
        TlsServer(identity, PeerKeyPolicy(accepts))

    /** Runs [desktop]'s connect on another thread and the phone's side of one connection here. */
    private fun connect(server: TlsServer, desktop: TestDesktop): Pair<Result<TlsTransport>, Future<SSLSocket>> {
        val listener = server.listen(port = 0, address = InetAddress.getLoopbackAddress()).also { listeners += it }
        val client = executor.submit<SSLSocket> {
            desktop.connect(listener.localPort).also { socket ->
                // A TLS 1.3 client finishes before the server has checked its
                // certificate; a read shows whether the phone kept the connection.
                socket.soTimeout = 5_000
                socket.outputStream.write(1)
                socket.outputStream.flush()
            }
        }
        val phoneSide = runCatching { server.secure(listener.accept()) }
        return phoneSide to client
    }

    @Test
    fun aClientThatSendsAByteNowAndThenIsCutOffAtTheHandshakeDeadline() {
        val server = server { true }
        val listener = server.listen(port = 0, address = InetAddress.getLoopbackAddress()).also { listeners += it }
        val dribbler = executor.submit {
            Socket(InetAddress.getLoopbackAddress(), listener.localPort).use { socket ->
                // A TLS record header that promises 512 bytes, then one byte every 200 ms:
                // each read finishes well within the per-read timeout.
                socket.getOutputStream().write(byteArrayOf(0x16, 0x03, 0x01, 0x02, 0x00))
                repeat(100) {
                    Thread.sleep(200)
                    socket.getOutputStream().write(1)
                }
            }
        }
        val accepted = listener.accept()

        val started = System.nanoTime()
        val result = runCatching { server.secure(accepted, handshakeTimeoutMillis = 1_000) }
        val tookMillis = (System.nanoTime() - started) / 1_000_000

        assertTrue(result.isFailure)
        assertTrue("The handshake took $tookMillis ms", tookMillis < 3_000)
        assertTrue(accepted.isClosed)
        dribbler.cancel(true)
    }

    @Test
    fun pairedDesktopConnectsAndBothSidesExportTheSameValue() {
        val desktop = TestDesktop(expectedPhone = phoneKey)
        val (phoneSide, client) = connect(server { it == desktop.fingerprint }, desktop)
        val transport = phoneSide.getOrThrow()
        val socket = client.get(10, TimeUnit.SECONDS)

        assertEquals(desktop.fingerprint, transport.peerKey)
        assertEquals(phoneKey, transport.localKey)
        assertEquals(phoneKey, desktop.seenPhone)
        assertEquals("TLSv1.3", socket.session.protocol)
        assertEquals("l2l/1", Conscrypt.getApplicationProtocol(socket))
        assertEquals(1, transport.input.read())

        val context = PairingCrypto.exporterContext(transport.peerKey, transport.localKey)
        val phoneEkm = transport.exportKeyingMaterial(PairingCrypto.QR_EXPORTER_LABEL, context, 32)
        val desktopEkm = desktop.export(socket, PairingCrypto.QR_EXPORTER_LABEL, context)
        assertNotNull(phoneEkm)
        assertArrayEquals(desktopEkm, phoneEkm)
        val sasEkm = transport.exportKeyingMaterial(PairingCrypto.SAS_EXPORTER_LABEL, context, 32)
        assertFalse(phoneEkm.contentEquals(sasEkm))

        transport.close()
        assertNull(transport.exportKeyingMaterial(PairingCrypto.QR_EXPORTER_LABEL, context, 32))
    }

    @Test
    fun unknownDesktopKeyIsRejectedDuringTheHandshake() {
        val desktop = TestDesktop()
        val (phoneSide, client) = connect(server { false }, desktop)

        assertTrue("not paired" in phoneSide.exceptionOrNull()?.message.orEmpty())
        assertTrue(runCatching { client.get(10, TimeUnit.SECONDS).inputStream.read() }.isFailure)
    }

    @Test
    fun desktopWithoutCertificateIsRejected() {
        val (phoneSide, client) = connect(server { true }, TestDesktop(identity = null))
        assertTrue(phoneSide.isFailure)
        assertTrue(runCatching { client.get(10, TimeUnit.SECONDS).inputStream.read() }.isFailure)
    }

    @Test
    fun desktopWithoutAlpnIsRejected() {
        val (phoneSide, _) = connect(server { true }, TestDesktop(alpn = null))
        assertRejectedForAlpn(phoneSide)
    }

    @Test
    fun desktopWithAnotherAlpnIsRejected() {
        val (phoneSide, _) = connect(server { true }, TestDesktop(alpn = arrayOf("h2")))
        // Conscrypt selects nothing when the offers do not overlap; the check after the handshake closes it.
        assertRejectedForAlpn(phoneSide)
    }

    private fun assertRejectedForAlpn(phoneSide: Result<TlsTransport>) {
        val message = phoneSide.exceptionOrNull()?.message.orEmpty()
        assertTrue("expected an ALPN rejection, got: $message", "ALPN" in message)
    }

    @Test
    fun tls12IsNeverNegotiated() {
        val (phoneSide, client) = connect(server { true }, TestDesktop(protocols = arrayOf("TLSv1.2")))
        assertTrue(phoneSide.isFailure)
        assertTrue(runCatching { client.get(10, TimeUnit.SECONDS) }.isFailure)
    }

    @Test
    fun everyConnectionRunsAFullHandshake() {
        val checks = AtomicInteger()
        val desktop = TestDesktop()
        val server = server { checks.incrementAndGet(); true }
        val first = connect(server, desktop)
        val second = connect(server, desktop) // same client context, so it would resume if it could

        assertEquals(2, checks.get())
        val sessions = listOf(first, second).map { it.second.get(10, TimeUnit.SECONDS).session }
        assertFalse(sessions[0].id.isNotEmpty() && sessions[0].id.contentEquals(sessions[1].id))
        first.first.getOrThrow().close()
        second.first.getOrThrow().close()
    }

    @Test
    fun aKeyThatCannotBeExportedSignsThroughAnotherProvider() {
        // The Keystore path: the TLS stack cannot read the key, so it asks
        // for NONEwithECDSA from another provider, as on the phone.
        OpaqueKeyProvider.install()
        val opaque = IdentityKey(OpaqueEcKey(phone.privateKey as ECPrivateKey), phone.certificate)
        val before = OpaqueKeyProvider.signatures
        val desktop = TestDesktop(expectedPhone = phoneKey)

        val (phoneSide, client) = connect(server(identity = { opaque }) { true }, desktop)

        phoneSide.getOrThrow().use { assertEquals(1, it.input.read()) }
        assertEquals(phoneKey, desktop.seenPhone)
        client.get(10, TimeUnit.SECONDS).close()
        assertEquals(before + 1, OpaqueKeyProvider.signatures)
    }

    @Test
    fun listenerMovesToAFreePortWhenTheDefaultIsTaken() {
        val taken = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).also { listeners += it }
        val server = server { true }

        val moved = server.listen(port = taken.localPort, address = InetAddress.getLoopbackAddress())
        listeners += moved
        assertNotEquals(taken.localPort, moved.localPort)

        val error = runCatching {
            server.listen(port = taken.localPort, address = InetAddress.getLoopbackAddress(), fallbackToAnyPort = false)
        }.exceptionOrNull()
        assertTrue("expected BindException, got $error", error is BindException)
    }
}
