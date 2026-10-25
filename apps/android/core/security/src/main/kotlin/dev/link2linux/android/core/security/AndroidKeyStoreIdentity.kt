package dev.link2linux.android.core.security

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import java.util.Date
import javax.security.auth.x500.X500Principal

/**
 * The phone's identity key, kept in the Android Keystore where it cannot be
 * exported. Created on first use under [alias]; clearing the app's data
 * removes it, after which the phone has to pair again.
 *
 * The key allows two digests. The TLS stack signs the handshake through the
 * Keystore with `NONEwithECDSA` over a digest it computed itself, which
 * needs [KeyProperties.DIGEST_NONE]; [KeyProperties.DIGEST_SHA256] covers
 * ordinary signatures. It needs no user authentication and no unlocked
 * screen, because the desktop connects while the phone sits locked in a
 * pocket.
 *
 * The certificate is the one the Keystore makes when it generates the key,
 * self-signed because the key may sign and has no attestation challenge.
 */
class AndroidKeyStoreIdentity(private val alias: String = DEFAULT_ALIAS) : IdentityKeySource {

    private val lock = Any()

    @Volatile
    private var cached: IdentityKey? = null

    override fun identity(): IdentityKey = cached ?: synchronized(lock) {
        cached ?: load().also { cached = it }
    }

    private fun load(): IdentityKey {
        val keyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }
        if (!keyStore.containsAlias(alias)) generate()
        val key = keyStore.getKey(alias, null) as PrivateKey
        val certificate = keyStore.getCertificate(alias) as X509Certificate
        return IdentityKey(key, certificate)
    }

    private fun generate() {
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256)
            .setCertificateSubject(X500Principal("CN=link2linux phone"))
            .setCertificateSerialNumber(BigInteger(63, SecureRandom()).setBit(62))
            // Peers ignore validity dates; a start in 1970 keeps a phone with a
            // wrong clock from tripping over its own certificate.
            .setCertificateNotBefore(Date(0))
            .setCertificateNotAfter(Date(NOT_AFTER_MILLIS))
            .build()
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, PROVIDER).run {
            initialize(spec)
            generateKeyPair()
        }
    }

    companion object {
        /** Keystore alias of the identity key. */
        const val DEFAULT_ALIAS: String = "l2l-identity-v1"

        private const val PROVIDER = "AndroidKeyStore"

        /** 2049-12-31T23:59:59Z, the last moment an X.509 UTCTime can express. */
        private const val NOT_AFTER_MILLIS = 2_524_607_999_000L
    }
}
