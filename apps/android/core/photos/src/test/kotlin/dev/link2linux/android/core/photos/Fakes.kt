package dev.link2linux.android.core.photos

import dev.link2linux.protocol.v1.Photo
import dev.link2linux.protocol.v1.photo
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map

/** A photo as a test lists it: taken at [taken], one per id. */
fun photo(id: String, taken: Long = 0, name: String = "$id.jpg", modified: Long = taken): Photo = photo {
    this.id = id
    takenUnixMs = taken
    modifiedUnixMs = modified
    this.name = name
    mimeType = "image/jpeg"
    size = 10
    width = 4000
    height = 3000
}

/**
 * The platform's photos as values the test sets. Each listed photo has a
 * decoded image of [width] x [height] and a file of [files] bytes, unless
 * the test removed it. Counts the change listeners it runs and the lists it
 * loads.
 */
class FakeLibrary(first: List<Photo>) : PhotoLibrary {
    val photos = MutableStateFlow(first)
    val files = HashMap<String, ByteArray>()
    val modified = HashMap<String, Long>()
    var width = 4000
    var height = 3000

    /** Bytes per pixel and quality point of the fake JPEGs: size = w * h * quality * this / 100. */
    var bytesPerPixel = 0.01

    var listeners = 0
    var loads = 0

    /** Thrown by [photos] while set. */
    var failure: Exception? = null

    /** While set, [decode] and [read] wait for it. */
    var gate: CompletableDeferred<Unit>? = null

    /** What [ScaledPhoto.jpeg] was asked for, in order: (longest side, quality). */
    val encoded = mutableListOf<Pair<Int, Int>>()
    var closed = 0

    override fun changes(): Flow<Unit> = flow {
        listeners++
        try {
            photos.map { }.collect { emit(Unit) }
        } finally {
            listeners--
        }
    }

    override suspend fun photos(limit: Int): List<Photo> {
        loads++
        failure?.let { throw it }
        // All of them, more than the limit too: the engine keeps to the limit itself.
        return photos.value
    }

    override suspend fun decode(photo: Photo, maxSize: Int): ScaledPhoto? {
        gate?.await()
        if (photos.value.none { it.id == photo.id }) return null
        val scale = minOf(1.0, maxSize.toDouble() / maxOf(width, height))
        return FakeScaled((width * scale).toInt(), (height * scale).toInt())
    }

    override suspend fun read(photo: Photo, offset: Long, maxBytes: Int): PhotoBytes? {
        gate?.await()
        val bytes = files[photo.id] ?: return null
        val from = minOf(offset, bytes.size.toLong()).toInt()
        val to = minOf(bytes.size.toLong(), offset + maxBytes).toInt().coerceAtLeast(from)
        return PhotoBytes(bytes.size.toLong(), modified[photo.id] ?: photo.modifiedUnixMs, bytes.copyOfRange(from, to))
    }

    private inner class FakeScaled(override val width: Int, override val height: Int) : ScaledPhoto {
        override fun jpeg(longestSide: Int, quality: Int): EncodedImage {
            encoded += longestSide to quality
            val scale = minOf(1.0, longestSide.toDouble() / maxOf(width, height))
            val w = (width * scale).toInt()
            val h = (height * scale).toInt()
            return EncodedImage(ByteArray((w * h * quality * bytesPerPixel / 100).toInt()), w, h)
        }

        override fun close() {
            closed++
        }
    }
}
