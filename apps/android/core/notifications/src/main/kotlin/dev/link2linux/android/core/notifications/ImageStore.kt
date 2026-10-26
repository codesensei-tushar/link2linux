package dev.link2linux.android.core.notifications

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ImageData
import dev.link2linux.protocol.v1.ImageRef
import dev.link2linux.protocol.v1.imageData
import dev.link2linux.protocol.v1.imageRef

/**
 * Images the phone has referenced, served by SHA-256 for `ImageRequest`.
 *
 * The cache is bounded. Images of notifications that are currently shared are
 * pinned (see [retain]) and are evicted only when the hard limit is passed, so
 * the desktop can fetch them for as long as the notification exists.
 */
class ImageStore(
    private val softLimitBytes: Long = 16L * 1024 * 1024,
    private val hardLimitBytes: Long = 48L * 1024 * 1024,
    private val maxMemoKeys: Int = 512,
) {
    private class Entry(val image: EncodedImage)

    private val lock = Any()

    // Access order: the first entry is the least recently used.
    private val entries = LinkedHashMap<ByteString, Entry>(64, 0.75f, true)
    private val memo = object : LinkedHashMap<String, ImageRef>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, ImageRef>?) = size > maxMemoKeys
    }
    private var pinned: Set<ByteString> = emptySet()
    private var totalBytes = 0L

    /**
     * Renders [source] for [kind], stores the bytes and returns the reference
     * to send, or null when there is no image or it cannot be sent (empty,
     * over 1 MiB, or no pixels).
     */
    fun intern(source: ImageSource?, kind: ImageKind): ImageRef? {
        if (source == null) return null
        val memoKey = source.cacheKey?.let { "${kind.name}:$it" }
        if (memoKey != null) {
            synchronized(lock) {
                val known = memo[memoKey]
                if (known != null && entries[known.sha256] != null) return known
            }
        }
        val image = try {
            source.render(kind)
        } catch (_: RuntimeException) {
            null
        } ?: return null
        if (image.data.isEmpty() || image.data.size > Limits.IMAGE_BYTES || image.width <= 0 || image.height <= 0) {
            return null
        }
        val hash = ByteString.copyFrom(WireText.sha256(image.data))
        val ref = imageRef {
            sha256 = hash
            width = image.width
            height = image.height
        }
        synchronized(lock) {
            if (entries[hash] == null) {
                entries[hash] = Entry(image)
                totalBytes += image.data.size
                evict()
            }
            if (memoKey != null) memo[memoKey] = ref
        }
        return ref
    }

    /** The image to answer an `ImageRequest` with, or null for NOT_FOUND. */
    fun get(sha256: ByteString): ImageData? {
        val entry = synchronized(lock) { entries[sha256] } ?: return null
        return imageData {
            this.sha256 = sha256
            format = entry.image.format
            data = ByteString.copyFrom(entry.image.data)
        }
    }

    /** Pins the images the shared notifications refer to; everything else may be evicted. */
    fun retain(hashes: Set<ByteString>) {
        synchronized(lock) {
            pinned = hashes
            evict()
        }
    }

    val sizeBytes: Long get() = synchronized(lock) { totalBytes }

    private fun evict() {
        // Unpinned images go first, least recently used first.
        if (totalBytes > softLimitBytes) {
            val it = entries.entries.iterator()
            while (it.hasNext() && totalBytes > softLimitBytes) {
                val e = it.next()
                if (e.key !in pinned) {
                    totalBytes -= e.value.image.data.size
                    it.remove()
                }
            }
        }
        if (totalBytes > hardLimitBytes) {
            val it = entries.entries.iterator()
            while (it.hasNext() && totalBytes > hardLimitBytes) {
                val e = it.next()
                totalBytes -= e.value.image.data.size
                it.remove()
            }
        }
    }
}
