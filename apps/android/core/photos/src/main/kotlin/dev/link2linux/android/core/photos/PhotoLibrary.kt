package dev.link2linux.android.core.photos

import dev.link2linux.protocol.v1.Photo
import kotlinx.coroutines.flow.Flow

/** Whether the phone can share its photos now, as its `FeatureStatus` reports it. */
enum class PhotoAvailability { READY, DISABLED, PERMISSION_REQUIRED }

/**
 * The phone's camera photos and screenshots, as the platform keeps them.
 * Every call may run on any thread, and several may run at once.
 */
interface PhotoLibrary {
    /**
     * Emits once as soon as it listens for changes, then whenever the
     * photos may have changed. Collecting starts the platform's listener,
     * and cancelling stops it. A false alarm costs one more [photos].
     */
    fun changes(): Flow<Unit>

    /**
     * The camera photos and screenshots, newest first, at most [limit].
     * Throws when the platform cannot list them, as without the permission.
     */
    suspend fun photos(limit: Int): List<Photo>

    /**
     * [photo] decoded upright, as it is shown, with its longest side at
     * most [maxSize] pixels and never larger than the photo itself. Null
     * when the photo is gone or can no longer be read.
     */
    suspend fun decode(photo: Photo, maxSize: Int): ScaledPhoto?

    /**
     * Up to [maxBytes] of [photo]'s file starting at [offset], with the
     * file's size and modified time now. The data is empty for an offset at
     * or past the end. Null when the photo is gone or can no longer be read.
     */
    suspend fun read(photo: Photo, offset: Long, maxBytes: Int): PhotoBytes?
}

/** A decoded photo, ready to encode at a smaller size. Holds the platform's image until closed. */
interface ScaledPhoto : AutoCloseable {
    val width: Int
    val height: Int

    /**
     * A JPEG of the photo with its longest side at most [longestSide]
     * pixels, scaled down to fit and never up, at [quality] (1 to 100).
     */
    fun jpeg(longestSide: Int, quality: Int): EncodedImage
}

/** An encoded image and its size in pixels. */
class EncodedImage(val data: ByteArray, val width: Int, val height: Int)

/** Bytes of a photo's file, with the file's [size] and modification time when they were read. */
class PhotoBytes(val size: Long, val modifiedUnixMs: Long, val data: ByteArray)
