package dev.link2linux.android.core.network

import dev.link2linux.android.core.common.Fingerprint
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import javax.net.ssl.SSLPeerUnverifiedException
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/** An established, already secured byte stream to a peer. */
interface Transport : Closeable {
    val input: InputStream
    val output: OutputStream
}

/**
 * TLS over TCP to a paired peer.
 *
 * Peers are reached by LAN address and present self-signed certificates, so
 * trust comes from pairing instead of hostnames: the connection is kept only
 * if the peer certificate's SHA-256 matches the fingerprint stored then.
 */
class TlsTransport private constructor(private val socket: SSLSocket) : Transport {

    override val input: InputStream = BufferedInputStream(socket.inputStream)
    override val output: OutputStream = BufferedOutputStream(socket.outputStream)

    override fun close() = socket.close()

    companion object {
        /**
         * Connects, completes the TLS handshake and checks the certificate pin.
         * Blocking; call it from an IO dispatcher.
         *
         * @param socketFactory carries this device's own certificate for the handshake.
         * @param pinnedSha256 the peer fingerprint saved at pairing (lowercase hex).
         */
        fun connect(
            address: InetSocketAddress,
            socketFactory: SSLSocketFactory,
            pinnedSha256: String,
            timeoutMillis: Int = 10_000,
        ): TlsTransport {
            val socket = socketFactory.createSocket() as SSLSocket
            try {
                socket.connect(address, timeoutMillis)
                socket.startHandshake()
                val certificate = socket.session.peerCertificates.firstOrNull()
                    ?: throw SSLPeerUnverifiedException("Peer sent no certificate")
                if (!Fingerprint.matches(pinnedSha256, certificate.encoded)) {
                    throw SSLPeerUnverifiedException("Peer certificate does not match the paired device")
                }
                return TlsTransport(socket)
            } catch (e: IOException) {
                socket.close()
                throw e
            }
        }
    }
}
