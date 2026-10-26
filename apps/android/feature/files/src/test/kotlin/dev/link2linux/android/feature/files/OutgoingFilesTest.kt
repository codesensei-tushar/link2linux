package dev.link2linux.android.feature.files

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.CancellationException

class OutgoingFilesTest {
    private val plenty = { Long.MAX_VALUE }

    @Test
    fun `a copy gives every byte`() {
        val data = ByteArray(300_000) { it.toByte() }
        val target = ByteArrayOutputStream()
        OutgoingFiles.copyBounded(ByteArrayInputStream(data), target, plenty, checkActive = {})
        assertArrayEquals(data, target.toByteArray())
    }

    @Test
    fun `a cancelled copy stops reading`() {
        var reads = 0
        val endless = object : InputStream() {
            override fun read() = 0

            override fun read(b: ByteArray, off: Int, len: Int): Int {
                reads++
                return len
            }
        }
        val target = ByteArrayOutputStream()
        var checks = 0
        assertThrows(CancellationException::class.java) {
            // Cancelled after the third piece.
            OutgoingFiles.copyBounded(endless, target, plenty, checkActive = { if (++checks > 3) throw CancellationException() })
        }
        assertEquals(3, reads)
        assertTrue(target.size() > 0)
    }

    @Test
    fun `a copy that leaves too little room stops`() {
        val data = ByteArray(9 * 1024 * 1024)
        assertThrows(IOException::class.java) {
            OutgoingFiles.copyBounded(ByteArrayInputStream(data), ByteArrayOutputStream(), free = { 0L }, checkActive = {})
        }
    }
}
