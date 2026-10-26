package dev.link2linux.android.core.network.tls

import java.security.PrivateKey
import java.security.Provider
import java.security.PublicKey
import java.security.Security
import java.security.Signature
import java.security.SignatureSpi
import java.security.interfaces.ECKey
import java.security.interfaces.ECPrivateKey
import java.security.spec.ECParameterSpec

/**
 * A private key whose bytes cannot be read, like an Android Keystore key:
 * no encoding, no scalar. The TLS stack can then only sign through
 * `Signature("NONEwithECDSA")` of another provider, which is the path it
 * takes with the phone's Keystore key.
 */
class OpaqueEcKey(internal val inner: ECPrivateKey) : PrivateKey, ECKey {
    override fun getAlgorithm() = "EC"
    override fun getFormat(): String? = null
    override fun getEncoded(): ByteArray? = null
    override fun getParams(): ECParameterSpec = inner.params
}

/**
 * Stands in for the Keystore's provider: offers `NONEwithECDSA` for
 * [OpaqueEcKey] only, and counts its signatures.
 */
object OpaqueKeyProvider : Provider("L2lOpaqueKeys", 1.0, "Signs with opaque test keys") {
    @Volatile
    var signatures = 0
        private set

    init {
        put("Signature.NONEwithECDSA", NoneWithEcdsa::class.java.name)
        put("Signature.NONEwithECDSA SupportedKeyClasses", OpaqueEcKey::class.java.name)
    }

    fun install() {
        if (Security.getProvider(name) == null) Security.addProvider(this)
    }

    class NoneWithEcdsa : SignatureSpi() {
        private var delegate: Signature? = null

        override fun engineInitSign(privateKey: PrivateKey?) {
            val key = privateKey as? OpaqueEcKey ?: throw java.security.InvalidKeyException("Opaque keys only")
            delegate = Signature.getInstance("NONEwithECDSA", "SunEC").apply { initSign(key.inner) }
        }

        override fun engineInitVerify(publicKey: PublicKey?) {
            throw UnsupportedOperationException()
        }
        override fun engineUpdate(b: Byte) {
            checkNotNull(delegate).update(b)
        }
        override fun engineUpdate(b: ByteArray, off: Int, len: Int) {
            checkNotNull(delegate).update(b, off, len)
        }
        override fun engineSign(): ByteArray = checkNotNull(delegate).sign().also { signatures++ }
        override fun engineVerify(sigBytes: ByteArray?): Boolean = throw UnsupportedOperationException()

        @Deprecated("Deprecated in Java")
        override fun engineSetParameter(param: String?, value: Any?) {
            throw UnsupportedOperationException()
        }

        @Deprecated("Deprecated in Java")
        override fun engineGetParameter(param: String?): Any = throw UnsupportedOperationException()
    }
}
