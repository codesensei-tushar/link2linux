package dev.link2linux.android.core.notifications

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ImageFormat
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ImageStoreTest {
    private class FakeImage(
        private val bytes: ByteArray,
        override val cacheKey: String? = null,
    ) : ImageSource {
        var renders = 0

        override fun render(kind: ImageKind): EncodedImage {
            renders++
            return EncodedImage(ImageFormat.IMAGE_FORMAT_PNG, bytes, 10, 20)
        }
    }

    private fun bytes(seed: Int, size: Int = 100) = ByteArray(size) { (it + seed).toByte() }

    @Test
    fun `the reference carries the sha256 and size, and the bytes are served by hash`() {
        val store = ImageStore()
        val data = bytes(1)
        val ref = store.intern(FakeImage(data), ImageKind.APP_ICON)!!
        assertArrayEquals(WireText.sha256(data), ref.sha256.toByteArray())
        assertEquals(10, ref.width)
        assertEquals(20, ref.height)
        val served = store.get(ref.sha256)!!
        assertArrayEquals(data, served.data.toByteArray())
        assertEquals(ImageFormat.IMAGE_FORMAT_PNG, served.format)
    }

    @Test
    fun `unknown hashes are not found`() {
        assertNull(ImageStore().get(ByteString.copyFrom(ByteArray(32))))
    }

    @Test
    fun `images over one MiB are refused`() {
        val store = ImageStore()
        assertNull(store.intern(FakeImage(ByteArray(Limits.IMAGE_BYTES + 1)), ImageKind.PICTURE))
        assertNull(store.intern(FakeImage(ByteArray(0)), ImageKind.PICTURE))
        assertNull(store.intern(null, ImageKind.PICTURE))
    }

    @Test
    fun `a cache key skips rendering the same image again`() {
        val store = ImageStore()
        val source = FakeImage(bytes(2), cacheKey = "pkg")
        store.intern(source, ImageKind.APP_ICON)
        store.intern(source, ImageKind.APP_ICON)
        assertEquals(1, source.renders)
    }

    @Test
    fun `unpinned images are evicted past the soft limit and pinned ones stay`() {
        val store = ImageStore(softLimitBytes = 250, hardLimitBytes = 10_000)
        val a = store.intern(FakeImage(bytes(1)), ImageKind.LARGE_ICON)!!
        store.retain(setOf(a.sha256))
        val b = store.intern(FakeImage(bytes(2)), ImageKind.LARGE_ICON)!!
        val c = store.intern(FakeImage(bytes(3)), ImageKind.LARGE_ICON)!!
        assertNotNull(store.get(a.sha256))
        assertNull(store.get(b.sha256))
        assertNotNull(store.get(c.sha256))
        assertTrue(store.sizeBytes <= 250)
    }

    @Test
    fun `the hard limit evicts even pinned images`() {
        val store = ImageStore(softLimitBytes = 100, hardLimitBytes = 150)
        val a = store.intern(FakeImage(bytes(1)), ImageKind.LARGE_ICON)!!
        val bHash = ByteString.copyFrom(WireText.sha256(bytes(2)))
        store.retain(setOf(a.sha256, bHash))
        val b = store.intern(FakeImage(bytes(2)), ImageKind.LARGE_ICON)!!
        assertNull(store.get(a.sha256))
        assertNotNull(store.get(b.sha256))
    }

    @Test
    fun `an image that is still rendering is pending, and the store says when it is ready`() {
        val waiting = mutableListOf<() -> Unit>()
        val store = ImageStore(renderer = { source, kind, done -> waiting += { done(source.render(kind)) } })
        var rendered = 0
        store.onRendered = { rendered++ }
        val source = FakeImage(bytes(4))

        assertNull(store.intern(source, ImageKind.LARGE_ICON))
        assertTrue(store.isPending(source, ImageKind.LARGE_ICON))
        // Asking again does not start a second render.
        assertNull(store.intern(source, ImageKind.LARGE_ICON))
        assertEquals(1, waiting.size)

        waiting.single()()
        assertEquals(1, rendered)
        assertFalse(store.isPending(source, ImageKind.LARGE_ICON))
        assertNotNull(store.intern(source, ImageKind.LARGE_ICON))
        assertEquals(1, source.renders)
    }

    @Test
    fun `an image that runs out of memory or overflows the stack is no image`() {
        val store = ImageStore()
        for (error in listOf(OutOfMemoryError(), StackOverflowError())) {
            val source = object : ImageSource {
                override fun render(kind: ImageKind): EncodedImage = throw error
            }
            assertNull(store.intern(source, ImageKind.PICTURE))
        }
    }
}
