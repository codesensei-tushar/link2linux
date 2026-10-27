package dev.link2linux.android.feature.photos

import dev.link2linux.protocol.v1.Photo
import dev.link2linux.protocol.v1.photo
import java.io.BufferedInputStream
import java.io.InputStream

/** One image row of MediaStore, as the query reads it. */
internal data class PhotoRow(
    val id: Long,
    val name: String?,
    val mimeType: String?,
    val size: Long,
    /** Null where the file does not say when it was taken. */
    val takenUnixMs: Long?,
    val addedUnixSeconds: Long,
    val modifiedUnixSeconds: Long,
    /** As stored in the file, before the rotation it asks for. */
    val width: Int,
    val height: Int,
    /** Clockwise degrees: 0, 90, 180 or 270. */
    val orientation: Int,
) {
    /**
     * The row as the protocol describes the photo: taken when it was added
     * where that is unknown, and sized as it is shown, so a photo turned on
     * its side swaps its width and height.
     */
    fun toPhoto(): Photo {
        val row = this
        val sideways = orientation == 90 || orientation == 270
        return photo {
            id = row.id.toString()
            takenUnixMs = row.takenUnixMs?.takeIf { it > 0 } ?: (row.addedUnixSeconds * MILLIS_PER_SECOND)
            modifiedUnixMs = row.modifiedUnixSeconds * MILLIS_PER_SECOND
            name = row.name.orEmpty()
            row.mimeType?.let { mimeType = it }
            size = row.size.coerceAtLeast(0)
            width = (if (sideways) row.height else row.width).coerceAtLeast(0)
            height = (if (sideways) row.width else row.height).coerceAtLeast(0)
        }
    }

    private companion object {
        const val MILLIS_PER_SECOND = 1_000L
    }
}

/** How photos are found and decoded, kept apart from the Android calls so JVM tests cover it. */
internal object PhotoQuery {

    /**
     * Where the camera and the screenshot tool save, as `RELATIVE_PATH`
     * patterns (Android 10 and later). `DCIM/%` covers `DCIM/Screenshots/`,
     * where some phones keep their screenshots.
     */
    val RELATIVE_PATHS: List<String> = listOf("DCIM/%", "Pictures/Screenshots/%")

    /**
     * The selection for Android 10 and later: in those folders, and no longer
     * being written. MediaStore leaves pending and trashed items out by
     * itself; the check on `is_pending` says so here too.
     */
    fun selection(relativePath: String, isPending: String): String =
        RELATIVE_PATHS.joinToString(" OR ", prefix = "(", postfix = ") AND $isPending = 0") { "$relativePath LIKE ?" }

    /** The selection for Android 8 and 9, by the files' paths under [roots]. */
    fun legacySelection(data: String, roots: List<String>): Pair<String, Array<String>> =
        roots.joinToString(" OR ") { "$data LIKE ?" } to roots.map { "${it.trimEnd('/')}/%" }.toTypedArray()

    /**
     * The `inSampleSize` that decodes an image whose longest side is
     * [longest] to no less than [maxSize] pixels, so scaling it down after
     * loses nothing: the largest power of 2 that leaves that much.
     */
    fun sampleSize(longest: Int, maxSize: Int): Int {
        var sample = 1
        while (longest / (sample * 2) >= maxSize) sample *= 2
        return sample
    }

    /** The size of [width] x [height] with its longest side at most [maxSize], never larger than it is. */
    fun fit(width: Int, height: Int, maxSize: Int): Pair<Int, Int> {
        val longest = maxOf(width, height)
        if (longest <= maxSize) return width to height
        val scale = maxSize.toDouble() / longest
        return maxOf(1, Math.round(width * scale).toInt()) to maxOf(1, Math.round(height * scale).toInt())
    }

    /** The photo's MediaStore id, or null for an id this phone never gives out. */
    fun mediaId(photo: Photo): Long? = photo.id.toLongOrNull()?.takeIf { it >= 0 && it.toString() == photo.id }

    /**
     * [source] able to rewind. The platform's decoder and EXIF reader mark
     * and reset the stream; a content stream cannot, and the decode then
     * fails or the reader refuses it.
     */
    fun markable(source: InputStream): InputStream = if (source.markSupported()) source else BufferedInputStream(source)
}
