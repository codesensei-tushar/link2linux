package dev.link2linux.android.core.network.tls

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.network.KeyingMaterialExporter
import dev.link2linux.android.core.network.Transport
import dev.link2linux.android.core.security.IdentityKeySource
import dev.link2linux.android.core.security.Spki
import org.conscrypt.Conscrypt
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.BindException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.Provider
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLException
import javax.net.ssl.SSLSocket

/**
 * The bundled Conscrypt, used as a provider object for link2linux's own
 * sockets on every API level. It is never installed as a default provider,
 * so the rest of the app keeps the platform's TLS.
 */
object ConscryptTls {
    val provider: Provider by lazy {
        check(Conscrypt.isAvailable()) { "The bundled Conscrypt could not load its native library" }
        Conscrypt.newProvider()
    }
}

/**
 * The phone's side of every connection: a TLS 1.3 server. The desktop dials
 * the phone and is the TLS client.
 *
 * - TLS 1.3 only, with a client certificate required and checked by key
 *   ([SpkiPinTrustManager] with [policy]).
 * - ALPN `l2l/1`; a connection that does not negotiate it is closed.
 * - No session tickets, so every connection runs a full handshake and
 *   proves both keys again.
 */
class TlsServer(
    identity: IdentityKeySource,
    policy: PeerKeyPolicy,
    provider: Provider = ConscryptTls.provider,
) {
    private val context: SSLContext = SSLContext.getInstance("TLSv1.3", provider).apply {
        init(arrayOf(IdentityKeyManager(identity)), arrayOf(SpkiPinTrustManager(policy)), SecureRandom())
    }

    /**
     * Opens the listening TCP socket on [port], or on any free port when that
     * one is taken and [fallbackToAnyPort] is set. Read the port from
     * [ServerSocket.getLocalPort].
     */
    fun listen(port: Int = DEFAULT_PORT, address: InetAddress? = null, fallbackToAnyPort: Boolean = true): ServerSocket {
        fun open(port: Int) = ServerSocket().apply {
            reuseAddress = true
            bind(InetSocketAddress(address, port), BACKLOG)
        }
        return try {
            open(port)
        } catch (e: BindException) {
            if (!fallbackToAnyPort || port == 0) throw e
            open(0)
        }
    }

    /**
     * Runs the TLS handshake as the server on an accepted connection, then
     * checks the protocol version and ALPN. Blocking; closes [socket] on
     * failure.
     *
     * @param handshakeTimeoutMillis how long the whole handshake may take.
     *   The socket is closed when it runs out, so a client that sends a byte
     *   now and then cannot hold the handshake open.
     * @param idleTimeoutMillis how long a read may wait once the connection
     *   is up. The desktop pings at least once a minute, so a longer silence
     *   means the connection is gone.
     */
    fun secure(socket: Socket, handshakeTimeoutMillis: Int = HANDSHAKE_TIMEOUT_MILLIS, idleTimeoutMillis: Int = IDLE_TIMEOUT_MILLIS): TlsTransport {
        // SO_TIMEOUT bounds each read, not the handshake; closing the socket does.
        val deadline = deadlines.schedule({ runCatching { socket.close() } }, handshakeTimeoutMillis.toLong(), TimeUnit.MILLISECONDS)
        try {
            return handshake(socket, handshakeTimeoutMillis, idleTimeoutMillis)
        } finally {
            deadline.cancel(false)
        }
    }

    private fun handshake(socket: Socket, handshakeTimeoutMillis: Int, idleTimeoutMillis: Int): TlsTransport {
        try {
            socket.soTimeout = handshakeTimeoutMillis
            val tls = context.socketFactory.createSocket(socket, socket.inetAddress.hostAddress, socket.port, true) as SSLSocket
            try {
                configure(tls)
                tls.startHandshake()
                val session = tls.session
                if (session.protocol != TLS_13) throw SSLException("Negotiated ${session.protocol}, not $TLS_13")
                val alpn = Conscrypt.getApplicationProtocol(tls)
                if (alpn != ALPN) throw SSLException("The desktop did not negotiate ALPN $ALPN (got $alpn)")
                val peer = session.peerCertificates.first() as X509Certificate
                val local = session.localCertificates.first() as X509Certificate
                socket.soTimeout = idleTimeoutMillis
                return TlsTransport(tls, peerKey = Spki.fingerprint(peer), localKey = Spki.fingerprint(local))
            } catch (e: Exception) {
                tls.close()
                throw e
            }
        } catch (e: Exception) {
            socket.close()
            throw if (e is IOException) e else SSLException("TLS handshake failed", e)
        }
    }

    private fun configure(socket: SSLSocket) {
        socket.useClientMode = false
        socket.enabledProtocols = arrayOf(TLS_13)
        // The default suites stay: Conscrypt picks the server key by the key
        // types of the TLS 1.2 suites, and only ECDHE_ECDSA ones yield "EC".
        check(socket.enabledCipherSuites.any { it.startsWith("TLS_ECDHE_ECDSA_") }) {
            "No ECDHE_ECDSA suite is enabled, so the identity key would never be offered"
        }
        socket.needClientAuth = true
        Conscrypt.setUseSessionTickets(socket, false)
        Conscrypt.setApplicationProtocols(socket, arrayOf(ALPN))
    }

    companion object {
        /** The phone's default TCP port. */
        const val DEFAULT_PORT: Int = 27380

        /** The ALPN protocol id both sides send. */
        const val ALPN: String = "l2l/1"

        const val HANDSHAKE_TIMEOUT_MILLIS: Int = 10_000

        /** Four times the longest interval at which the desktop pings. */
        const val IDLE_TIMEOUT_MILLIS: Int = 240_000

        private const val TLS_13 = "TLSv1.3"
        private const val BACKLOG = 8

        /** Closes sockets whose handshake ran out of time. Its task never blocks. */
        private val deadlines: ScheduledExecutorService by lazy {
            Executors.newSingleThreadScheduledExecutor { task -> Thread(task, "l2l-tls-deadline").apply { isDaemon = true } }
        }
    }
}

/**
 * An established TLS connection from a desktop, with its key already
 * checked against the [PeerKeyPolicy].
 */
class TlsTransport internal constructor(
    private val socket: SSLSocket,
    /** `fp_D`: the key the desktop presented in this handshake. */
    val peerKey: KeyFingerprint,
    /** `fp_P`: the key this phone presented in this handshake. */
    val localKey: KeyFingerprint,
) : Transport, KeyingMaterialExporter {

    /** Where the desktop connected from. */
    val remoteAddress: InetSocketAddress = socket.remoteSocketAddress as InetSocketAddress

    override val input: InputStream = BufferedInputStream(socket.inputStream)
    override val output: OutputStream = BufferedOutputStream(socket.outputStream)

    override fun exportKeyingMaterial(label: String, context: ByteArray, length: Int): ByteArray? = try {
        Conscrypt.exportKeyingMaterial(socket, label, context, length)
    } catch (e: SSLException) {
        null
    }

    override fun close() = socket.close()
}
