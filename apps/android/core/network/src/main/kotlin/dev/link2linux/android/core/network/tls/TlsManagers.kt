package dev.link2linux.android.core.network.tls

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.security.IdentityKeySource
import dev.link2linux.android.core.security.Spki
import java.net.Socket
import java.security.Principal
import java.security.PrivateKey
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509ExtendedTrustManager

/**
 * Decides which desktops may connect, by key alone: the current or next key
 * of a paired desktop, the key from a scanned QR code, or any key while
 * pairing without a camera. Called on the handshake thread, so it must not
 * block.
 */
fun interface PeerKeyPolicy {
    fun accepts(key: KeyFingerprint): Boolean
}

/**
 * Checks the desktop's client certificate by the fingerprint of its key and
 * nothing else: names, dates, extensions and the certificate's own
 * signature are ignored. Only canonical P-256 keys pass.
 *
 * The TLS stack verifies the CertificateVerify signature itself, so a
 * desktop that passes here has also proven that it holds the key.
 *
 * [getAcceptedIssuers] is empty, so the certificate request names no
 * certificate authorities.
 */
class SpkiPinTrustManager(private val policy: PeerKeyPolicy) : X509ExtendedTrustManager() {

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) = check(chain)

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) =
        check(chain)

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) =
        check(chain)

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        serverOnly()
    }

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) {
        serverOnly()
    }

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) {
        serverOnly()
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

    private fun check(chain: Array<out X509Certificate>?) {
        val leaf = chain?.firstOrNull() ?: throw CertificateException("The desktop sent no certificate")
        val key = Spki.fingerprint(leaf)
        if (!policy.accepts(key)) throw CertificateException("Key $key is not paired with this phone")
    }

    private fun serverOnly(): Nothing = throw CertificateException("The phone never connects as a TLS client")
}

/**
 * Hands the TLS stack the phone's identity key for the server side of the
 * handshake.
 *
 * The TLS stack asks for a server key once per key type of the enabled TLS
 * 1.2 cipher suites, even when only TLS 1.3 is enabled; this answers "EC"
 * and nothing else. That is why the ECDHE_ECDSA suites must stay enabled.
 */
class IdentityKeyManager(private val identity: IdentityKeySource) : X509ExtendedKeyManager() {

    override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?): String? =
        ALIAS.takeIf { keyType == KEY_TYPE }

    override fun chooseEngineServerAlias(keyType: String?, issuers: Array<out Principal>?, engine: SSLEngine?): String? =
        ALIAS.takeIf { keyType == KEY_TYPE }

    override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? =
        if (keyType == KEY_TYPE) arrayOf(ALIAS) else null

    override fun getCertificateChain(alias: String?): Array<X509Certificate>? =
        if (alias == ALIAS) arrayOf(identity.identity().certificate) else null

    override fun getPrivateKey(alias: String?): PrivateKey? =
        if (alias == ALIAS) identity.identity().privateKey else null

    override fun chooseClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?): String? =
        null

    override fun chooseEngineClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, engine: SSLEngine?): String? =
        null

    override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? = null

    private companion object {
        const val ALIAS = "identity"
        const val KEY_TYPE = "EC"
    }
}
