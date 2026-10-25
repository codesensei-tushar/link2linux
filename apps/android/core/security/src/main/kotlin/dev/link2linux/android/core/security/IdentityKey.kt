package dev.link2linux.android.core.security

import dev.link2linux.android.core.model.KeyFingerprint
import java.security.PrivateKey
import java.security.cert.X509Certificate

/**
 * This device's identity: an ECDSA P-256 private key and a self-signed
 * certificate for it. Peers know the device by [fingerprint] alone; nothing
 * else in the certificate matters to them.
 */
class IdentityKey(val privateKey: PrivateKey, val certificate: X509Certificate) {

    /** `fp_P`: the fingerprint of this device's key. */
    val fingerprint: KeyFingerprint = Spki.fingerprint(certificate)

    override fun toString(): String = "IdentityKey($fingerprint)"
}

/**
 * Where the identity key comes from: the Android Keystore in the app
 * ([AndroidKeyStoreIdentity]), a software key in JVM tests.
 */
fun interface IdentityKeySource {

    /**
     * Returns the identity, creating it on first use. Blocking: Keystore
     * calls go to another process, so call it off the main thread.
     */
    fun identity(): IdentityKey
}
