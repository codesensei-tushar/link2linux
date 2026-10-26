package dev.link2linux.android.core.security

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Seals data at rest so that only this installation can read it back, and
 * notices any change to it. [associatedData] is authenticated but not
 * stored: pass the same bytes to [open], for example a file's purpose.
 */
interface SecretBox {
    fun seal(plaintext: ByteArray, associatedData: ByteArray): ByteArray

    /**
     * @throws javax.crypto.AEADBadTagException if the data was changed or
     *   sealed with another key; another [GeneralSecurityException] if the
     *   key could not be used this time.
     */
    fun open(sealed: ByteArray, associatedData: ByteArray): ByteArray
}

/**
 * AES-256-GCM. A sealed value is the 12-byte IV followed by the ciphertext
 * and its 16-byte tag; the cipher picks the IV.
 */
open class AesGcmSecretBox(private val key: () -> SecretKey) : SecretBox {

    override fun seal(plaintext: ByteArray, associatedData: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(associatedData)
        val iv = cipher.iv
        check(iv.size == IV_SIZE) { "Unexpected IV size ${iv.size}" }
        return iv + cipher.doFinal(plaintext)
    }

    override fun open(sealed: ByteArray, associatedData: ByteArray): ByteArray {
        // As for a failed tag: these bytes can never be opened.
        if (sealed.size < IV_SIZE + TAG_BITS / 8) throw AEADBadTagException("Sealed data is too short")
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, sealed, 0, IV_SIZE))
        cipher.updateAAD(associatedData)
        return cipher.doFinal(sealed, IV_SIZE, sealed.size - IV_SIZE)
    }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_SIZE = 12
        const val TAG_BITS = 128
    }
}

/**
 * [AesGcmSecretBox] with its key in the Android Keystore, created on first
 * use. The key never leaves the Keystore, so data sealed with it cannot be
 * read on another device or after the app's data is cleared.
 */
class KeystoreSecretBox(alias: String = DEFAULT_ALIAS) : AesGcmSecretBox({ keystoreKey(alias) }) {

    companion object {
        /** Keystore alias of the storage key. */
        const val DEFAULT_ALIAS: String = "l2l-storage-v1"

        private const val PROVIDER = "AndroidKeyStore"

        private fun keystoreKey(alias: String): SecretKey = synchronized(KeystoreSecretBox::class.java) {
            val keyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }
            (keyStore.getKey(alias, null) as SecretKey?) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER).run {
                init(
                    KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .build(),
                )
                generateKey()
            }
        }
    }
}
