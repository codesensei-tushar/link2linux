package dev.link2linux.android.core.security

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.GeneralSecurityException
import javax.crypto.KeyGenerator

/** The sealing logic with a software key; on a phone the key lives in the Keystore. */
class AesGcmSecretBoxTest {

    private fun newKey() = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()

    private val key = newKey()
    private val box = AesGcmSecretBox { key }
    private val purpose = "pairings".toByteArray()

    @Test
    fun opensWhatItSealed() {
        val plaintext = "presence keys".toByteArray()
        val sealed = box.seal(plaintext, purpose)
        assertFalse(sealed.toList().windowed(plaintext.size).contains(plaintext.toList()))
        assertArrayEquals(plaintext, box.open(sealed, purpose))
    }

    @Test
    fun everySealUsesAFreshIv() {
        val a = box.seal(ByteArray(8), purpose)
        val b = box.seal(ByteArray(8), purpose)
        assertFalse(a.contentEquals(b))
    }

    @Test
    fun changedDataIsRejected() {
        val sealed = box.seal("abc".toByteArray(), purpose)
        for (index in sealed.indices) {
            val changed = sealed.copyOf().also { it[index] = (it[index].toInt() xor 1).toByte() }
            assertRejected { box.open(changed, purpose) }
        }
        assertRejected { box.open(sealed.copyOf(sealed.size - 1), purpose) }
        assertRejected { box.open(ByteArray(10), purpose) }
    }

    @Test
    fun otherAssociatedDataOrKeyIsRejected() {
        val sealed = box.seal("abc".toByteArray(), purpose)
        assertRejected { box.open(sealed, "other".toByteArray()) }
        val otherKey = newKey()
        assertRejected { AesGcmSecretBox { otherKey }.open(sealed, purpose) }
    }

    private fun assertRejected(block: () -> Unit) {
        val error = runCatching(block).exceptionOrNull()
        assertTrue("expected GeneralSecurityException, got $error", error is GeneralSecurityException)
    }
}
