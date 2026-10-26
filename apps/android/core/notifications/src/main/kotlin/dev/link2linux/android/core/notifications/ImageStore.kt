package dev.link2linux.android.core.notifications

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ImageData
import dev.link2linux.protocol.v1.ImageRef
import dev.link2linux.protocol.v1.imageData
import dev.link2linux.protocol.v1.imageRef
import java.util.EnumMap
import java.util.WeakHashMap
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Where images are rendered. Rendering loads another app's image, which can
 * be slow, never finish or be huge, so the app renders on threads of its own
 * ([ImageRenderers.background]); tests render at once ([ImageRenderers.inline]).
 */
fun interface ImageRenderer {
    /** Renders [source] for [kind], now or later, and hands the result, or null, to [done]. */
    fun render(source: ImageSource, kind: ImageKind, done: (EncodedImage?) -> Unit)
}

object ImageRenderers {
    /** On the caller's thread, before [ImageRenderer.render] returns. */
    val inline: ImageRenderer = ImageRenderer { source, kind, done -> done(renderSafely(source, kind)) }

    /** On [threads] threads of their own; at most [maxQueued] renders wait, and any beyond get no image. */
    fun background(threads: Int = 2, maxQueued: Int = 256): ImageRenderer {
        val executor = ThreadPoolExecutor(threads, threads, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue(maxQueued)) { task ->
            Thread(task, "l2l-images").apply { isDaemon = true }
        }
        return ImageRenderer { source, kind, done ->
            try {
                executor.execute { done(renderSafely(source, kind)) }
            } catch (_: RejectedExecutionException) {
                done(null)
            }
        }
    }

    /** Never throws: an image that fails in any way, even by running out of memory, is no image. */
    fun renderSafely(source: ImageSource, kind: ImageKind): EncodedImage? = try {
        source.render(kind)
    } catch (_: OutOfMemoryError) {
        null
    } catch (_: Exception) {
        null
    } catch (_: StackOverflowError) {
        null
    }
}

/**
 * Images the phone has referenced, served by SHA-256 for `ImageRequest`.
 *
 * The cache is bounded. Images of notifications that are currently shared are
 * pinned (see [retain]) and are evicted only when the hard limit is passed, so
 * the desktop can fetch them for as long as the notification exists.
 *
 * [intern] never waits for a render: an image that is not ready yet is
 * rendered by [renderer], and [onRendered] says when it is, so the caller can
 * ask again.
 */
class ImageStore(
    private val softLimitBytes: Long = 16L * 1024 * 1024,
    private val hardLimitBytes: Long = 48L * 1024 * 1024,
    private val maxMemoKeys: Int = 512,
    private val renderer: ImageRenderer = ImageRenderers.inline,
) {
    private class Entry(val image: EncodedImage)

    /** A render that [intern] started: not finished yet, or finished with this reference (null for no image). */
    private class Outcome(@Volatile var done: Boolean = false, @Volatile var ref: ImageRef? = null)

    /** Called on the renderer's thread when an image [intern] could not return yet has finished rendering. */
    @Volatile
    var onRendered: () -> Unit = {}

    private val lock = Any()

    // By source object: a notification's sources live as long as its snapshot.
    private val outcomes = WeakHashMap<ImageSource, EnumMap<ImageKind, Outcome>>()

    // Access order: the first entry is the least recently used.
    private val entries = LinkedHashMap<ByteString, Entry>(64, 0.75f, true)
    private val memo = object : LinkedHashMap<String, ImageRef>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, ImageRef>?) = size > maxMemoKeys
    }
    private var pinned: Set<ByteString> = emptySet()
    private var totalBytes = 0L

    /**
     * The reference to send for [source] as [kind], or null when there is no
     * image, it cannot be sent (empty, over 1 MiB, or no pixels), or it is
     * still rendering (see [isPending]).
     */
    fun intern(source: ImageSource?, kind: ImageKind): ImageRef? {
        if (source == null) return null
        val memoKey = source.cacheKey?.let { "${kind.name}:$it" }
        val outcome = synchronized(lock) {
            if (memoKey != null) {
                val known = memo[memoKey]
                if (known != null && entries[known.sha256] != null) return known
            }
            val forSource = outcomes.getOrPut(source) { EnumMap(ImageKind::class.java) }
            val known = forSource[kind]
            val ref = known?.ref
            when {
                known == null -> Unit
                !known.done || ref == null -> return null // still rendering, or no image
                entries[ref.sha256] != null -> return ref
                else -> Unit // evicted since: render it again
            }
            Outcome().also { forSource[kind] = it }
        }
        val returned = AtomicBoolean(false)
        renderer.render(source, kind) { image ->
            outcome.ref = image?.let { keep(it, memoKey) }
            outcome.done = true
            if (returned.get()) onRendered()
        }
        returned.set(true)
        return outcome.ref.takeIf { outcome.done }
    }

    /** Whether [source] as [kind] is still rendering, so [intern] has no answer for it yet. */
    fun isPending(source: ImageSource?, kind: ImageKind): Boolean {
        if (source == null) return false
        return synchronized(lock) { outcomes[source]?.get(kind)?.done == false }
    }

    /** Stores a rendered [image] and returns its reference, or null if it cannot be sent. */
    private fun keep(image: EncodedImage, memoKey: String?): ImageRef? {
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
