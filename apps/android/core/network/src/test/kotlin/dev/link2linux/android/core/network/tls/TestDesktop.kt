package dev.link2linux.android.core.network.tls

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.security.IdentityKey
import dev.link2linux.android.core.security.IdentityKeySource
import dev.link2linux.android.core.security.Spki
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.android.core.testing.TestIdentity
import org.conscrypt.Conscrypt
import java.net.InetAddress
import java.net.Socket
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.KeyManager
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509ExtendedTrustManager

/** A software identity for the phone side of JVM tests. */
fun TestIdentity.asSource(): IdentityKeySource {
    val key = IdentityKey(privateKey, certificate)
    return IdentityKeySource { key }
}

/**
 * The desktop's side of a connection in tests: a TLS 1.3 client with a
 * software P-256 key, written independently of the phone's code. It records
 * the phone's key and, when [expectedPhone] is set, refuses any other.
 */
class TestDesktop(
    val identity: TestIdentity? = TestCertificates.generate("test desktop"),
    private val expectedPhone: KeyFingerprint? = null,
    private val alpn: Array<String>? = arrayOf(TlsServer.ALPN),
    private val protocols: Array<String> = arrayOf("TLSv1.3"),
) {
    val fingerprint: KeyFingerprint? = identity?.let { Spki.fingerprint(it.certificate) }

    @Volatile
    var seenPhone: KeyFingerprint? = null
        private set

    val context: SSLContext = SSLContext.getInstance("TLS", Conscrypt.newProvider()).apply {
        init(identity?.let { arrayOf<KeyManager>(ClientKeyManager(it)) }, arrayOf(PhoneTrustManager()), SecureRandom())
    }

    /** Connects and completes the handshake from the client's point of view. */
    fun connect(port: Int): SSLSocket {
        val socket = context.socketFactory.createSocket(InetAddress.getLoopbackAddress(), port) as SSLSocket
        socket.soTimeout = 10_000
        socket.enabledProtocols = protocols
        alpn?.let { Conscrypt.setApplicationProtocols(socket, it) }
        socket.startHandshake()
        return socket
    }

    fun export(socket: SSLSocket, label: String, context: ByteArray): ByteArray? =
        Conscrypt.exportKeyingMaterial(socket, label, context, 32)

    private inner class PhoneTrustManager : X509ExtendedTrustManager() {
        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String?) {
            val key = Spki.fingerprint(chain.first())
            if (expectedPhone != null && key != expectedPhone) throw CertificateException("Not the expected phone")
            seenPhone = key
        }

        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String?, socket: Socket?) =
            checkServerTrusted(chain, authType)

        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String?, engine: SSLEngine?) =
            checkServerTrusted(chain, authType)

        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {
            throw CertificateException("client only")
        }

        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) {
            throw CertificateException("client only")
        }

        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) {
            throw CertificateException("client only")
        }

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    private class ClientKeyManager(private val identity: TestIdentity) : X509ExtendedKeyManager() {
        override fun chooseClientAlias(types: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?) = "desktop"
        override fun chooseEngineClientAlias(types: Array<out String>?, issuers: Array<out Principal>?, engine: SSLEngine?) =
            "desktop"
        override fun getClientAliases(type: String?, issuers: Array<out Principal>?) = arrayOf("desktop")
        override fun getCertificateChain(alias: String?) = arrayOf(identity.certificate)
        override fun getPrivateKey(alias: String?): PrivateKey = identity.privateKey
        override fun chooseServerAlias(type: String?, issuers: Array<out Principal>?, socket: Socket?): String? = null
        override fun getServerAliases(type: String?, issuers: Array<out Principal>?): Array<String>? = null
    }
}
