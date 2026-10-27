package dev.link2linux.android.feature.photos

import android.annotation.SuppressLint
import android.content.ContentResolver
import android.content.ContentUris
import android.content.Context
import android.database.ContentObserver
import android.database.Cursor
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.util.Size
import androidx.annotation.RequiresApi
import androidx.core.graphics.scale
import dev.link2linux.android.core.photos.EncodedImage
import dev.link2linux.android.core.photos.PhotoBytes
import dev.link2linux.android.core.photos.PhotoLibrary
import dev.link2linux.android.core.photos.ScaledPhoto
import dev.link2linux.protocol.v1.Photo
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.FileNotFoundException
import java.io.IOException
import java.nio.ByteBuffer

/**
 * The phone's camera photos and screenshots, read through MediaStore: the
 * newest first, by when they were taken. It reads only what the app's
 * permission lets it see, which may be only the photos the user picked.
 *
 * [onRefused] hears when Android refused a read, as after the user took the
 * permission back, so the feature's state can follow.
 */
internal class AndroidPhotoLibrary(context: Context, private val onRefused: () -> Unit) : PhotoLibrary {
    private val resolver: ContentResolver = context.contentResolver
    private val collection: Uri = MediaStore.Images.Media.EXTERNAL_CONTENT_URI

    override fun changes(): Flow<Unit> = callbackFlow {
        // No handler: onChange runs on a binder thread, which only signals.
        val observer = object : ContentObserver(null) {
            override fun onChange(selfChange: Boolean) {
                trySend(Unit)
            }
        }
        resolver.registerContentObserver(collection, true, observer)
        // Listening from here on, so a change made while the list loads is not missed.
        trySend(Unit)
        awaitClose { resolver.unregisterContentObserver(observer) }
    }.conflate()

    override suspend fun photos(limit: Int): List<Photo> = withContext(Dispatchers.IO) {
        val cursor = refusedOnSecurity { query(limit) } ?: error("MediaStore gave no cursor")
        cursor.use { rows ->
            val columns = Columns(rows)
            buildList {
                while (rows.moveToNext() && size < limit) add(columns.row(rows).toPhoto())
            }
        }
    }

    override suspend fun decode(photo: Photo, maxSize: Int): ScaledPhoto? = withContext(Dispatchers.IO) {
        val uri = uriOf(photo) ?: return@withContext null
        val decoded = try {
            when {
                // MediaStore keeps small thumbnails ready; larger previews come from the original.
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && maxSize <= CACHED_THUMBNAIL_MAX ->
                    resolver.loadThumbnail(uri, Size(maxSize, maxSize), null)
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.P -> decodeImage(uri, maxSize)
                else -> decodeSampled(uri, maxSize)
            }
        } catch (_: FileNotFoundException) {
            null
        } catch (_: SecurityException) {
            onRefused()
            null
        } ?: return@withContext null
        BitmapPhoto(scaledToFit(decoded, maxSize))
    }

    override suspend fun read(photo: Photo, offset: Long, maxBytes: Int): PhotoBytes? = withContext(Dispatchers.IO) {
        val uri = uriOf(photo) ?: return@withContext null
        try {
            // From the same row as the list, so an unchanged photo reads with the time the desktop has.
            val modified = modifiedUnixMs(uri) ?: return@withContext null
            val descriptor = resolver.openFileDescriptor(uri, "r") ?: return@withContext null
            ParcelFileDescriptor.AutoCloseInputStream(descriptor).use { input ->
                val channel = input.channel
                val size = channel.size()
                val length = if (offset >= size) 0 else minOf(maxBytes.toLong(), size - offset).toInt()
                val buffer = ByteBuffer.allocate(length)
                while (buffer.hasRemaining()) {
                    if (channel.read(buffer, offset + buffer.position()) < 0) break
                }
                PhotoBytes(size, modified, buffer.array().copyOf(buffer.position()))
            }
        } catch (_: FileNotFoundException) {
            null
        } catch (_: SecurityException) {
            onRefused()
            null
        }
    }

    private fun query(limit: Int): Cursor? {
        val sortColumn = MediaStore.MediaColumns.DATE_TAKEN
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val args = Bundle().apply {
                putString(ContentResolver.QUERY_ARG_SQL_SELECTION, PhotoQuery.selection(RELATIVE_PATH, IS_PENDING))
                putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, PhotoQuery.RELATIVE_PATHS.toTypedArray())
                putStringArray(ContentResolver.QUERY_ARG_SORT_COLUMNS, arrayOf(sortColumn))
                putInt(ContentResolver.QUERY_ARG_SORT_DIRECTION, ContentResolver.QUERY_SORT_DIRECTION_DESCENDING)
                putInt(ContentResolver.QUERY_ARG_LIMIT, limit)
            }
            return resolver.query(collection, PROJECTION, args, null)
        }
        // Before Android 11, MediaStore takes the limit as part of the sort order.
        val order = "$sortColumn DESC LIMIT $limit"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val selection = PhotoQuery.selection(RELATIVE_PATH, IS_PENDING)
            return resolver.query(collection, PROJECTION, selection, PhotoQuery.RELATIVE_PATHS.toTypedArray(), order)
        }
        val (selection, args) = PhotoQuery.legacySelection(LEGACY_DATA, legacyRoots())
        return resolver.query(collection, PROJECTION, selection, args, order)
    }

    private fun modifiedUnixMs(uri: Uri): Long? =
        resolver.query(uri, arrayOf(MediaStore.MediaColumns.DATE_MODIFIED), null, null, null)?.use { rows ->
            if (rows.moveToFirst()) rows.getLong(0) * MILLIS_PER_SECOND else null
        }

    /** The photo's content URI, from its MediaStore id: the photo is one the engine listed. */
    private fun uriOf(photo: Photo): Uri? = PhotoQuery.mediaId(photo)?.let { ContentUris.withAppendedId(collection, it) }

    /** A decode of the original at no less than [maxSize]; ImageDecoder turns it upright. */
    @RequiresApi(Build.VERSION_CODES.P)
    private fun decodeImage(uri: Uri, maxSize: Int): Bitmap =
        ImageDecoder.decodeBitmap(ImageDecoder.createSource(resolver, uri)) { decoder, info, _ ->
            // Software, so it can be scaled and compressed.
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            decoder.setTargetSampleSize(PhotoQuery.sampleSize(maxOf(info.size.width, info.size.height), maxSize))
        }

    /**
     * A decode of the original at no less than [maxSize], turned as its EXIF
     * data asks, for Android 8. The platform's ExifInterface reads the
     * orientation right from Android 7 on, so the library would add a
     * dependency for nothing.
     */
    @SuppressLint("ExifInterface")
    private fun decodeSampled(uri: Uri, maxSize: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(PhotoQuery.markable(it), null, bounds) } ?: return null
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        val options = BitmapFactory.Options().apply {
            inSampleSize = PhotoQuery.sampleSize(maxOf(bounds.outWidth, bounds.outHeight), maxSize)
        }
        val decoded = resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(PhotoQuery.markable(it), null, options) } ?: return null
        val degrees = resolver.openInputStream(uri)?.use { stream ->
            try {
                rotationOf(ExifInterface(PhotoQuery.markable(stream)))
            } catch (_: IOException) {
                // A photo with no readable orientation is shown as stored.
                0
            } catch (_: IllegalArgumentException) {
                0
            }
        } ?: 0
        if (degrees == 0) return decoded
        val turned = Bitmap.createBitmap(decoded, 0, 0, decoded.width, decoded.height, Matrix().apply { postRotate(degrees.toFloat()) }, true)
        if (turned !== decoded) decoded.recycle()
        return turned
    }

    @SuppressLint("ExifInterface")
    private fun rotationOf(exif: ExifInterface): Int = when (exif.getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
        ExifInterface.ORIENTATION_ROTATE_90 -> 90
        ExifInterface.ORIENTATION_ROTATE_180 -> 180
        ExifInterface.ORIENTATION_ROTATE_270 -> 270
        else -> 0
    }

    /** [bitmap] with its longest side at most [maxSize]; a decode may come out larger than asked. */
    private fun scaledToFit(bitmap: Bitmap, maxSize: Int): Bitmap {
        val (width, height) = PhotoQuery.fit(bitmap.width, bitmap.height, maxSize)
        if (width == bitmap.width && height == bitmap.height) return bitmap
        val scaled = bitmap.scale(width, height)
        if (scaled !== bitmap) bitmap.recycle()
        return scaled
    }

    /** Runs [block], and tells [onRefused] when Android refuses it. */
    private inline fun <T> refusedOnSecurity(block: () -> T): T = try {
        block()
    } catch (e: SecurityException) {
        onRefused()
        throw e
    }

    /** The column indexes of the query's projection. */
    private class Columns(rows: Cursor) {
        private val id = rows.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
        private val name = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.DISPLAY_NAME)
        private val mimeType = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.MIME_TYPE)
        private val size = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.SIZE)
        private val taken = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_TAKEN)
        private val added = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_ADDED)
        private val modified = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_MODIFIED)
        private val width = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.WIDTH)
        private val height = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.HEIGHT)
        private val orientation = rows.getColumnIndexOrThrow(MediaStore.MediaColumns.ORIENTATION)

        fun row(rows: Cursor) = PhotoRow(
            id = rows.getLong(id),
            name = rows.getString(name),
            mimeType = rows.getString(mimeType),
            size = rows.getLong(size),
            takenUnixMs = if (rows.isNull(taken)) null else rows.getLong(taken),
            addedUnixSeconds = rows.getLong(added),
            modifiedUnixSeconds = rows.getLong(modified),
            width = rows.getInt(width),
            height = rows.getInt(height),
            orientation = rows.getInt(orientation),
        )
    }

    /** A decoded photo; scaled and compressed on the engine's thread, recycled when closed. */
    private class BitmapPhoto(private val bitmap: Bitmap) : ScaledPhoto {
        override val width: Int = bitmap.width
        override val height: Int = bitmap.height

        override fun jpeg(longestSide: Int, quality: Int): EncodedImage {
            val (width, height) = PhotoQuery.fit(bitmap.width, bitmap.height, longestSide)
            val scaled = if (width == bitmap.width && height == bitmap.height) bitmap else bitmap.scale(width, height)
            val out = ByteArrayOutputStream()
            try {
                check(scaled.compress(Bitmap.CompressFormat.JPEG, quality, out)) { "The JPEG encoder failed" }
            } finally {
                if (scaled !== bitmap) scaled.recycle()
            }
            return EncodedImage(out.toByteArray(), width, height)
        }

        override fun close() = bitmap.recycle()
    }

    private companion object {
        /** The largest thumbnail taken from MediaStore's own; it keeps them at about 512 pixels. */
        const val CACHED_THUMBNAIL_MAX = 512

        const val MILLIS_PER_SECOND = 1_000L

        // Column names of Android 10 and later; the queries that use them run there only.
        const val RELATIVE_PATH = "relative_path"
        const val IS_PENDING = "is_pending"

        /** The file's path, which Android 8 and 9 still give. */
        const val LEGACY_DATA = "_data"

        val PROJECTION = arrayOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.MIME_TYPE,
            MediaStore.MediaColumns.SIZE,
            MediaStore.MediaColumns.DATE_TAKEN,
            MediaStore.MediaColumns.DATE_ADDED,
            MediaStore.MediaColumns.DATE_MODIFIED,
            MediaStore.MediaColumns.WIDTH,
            MediaStore.MediaColumns.HEIGHT,
            MediaStore.MediaColumns.ORIENTATION,
        )

        /** Where Android 8 and 9 save camera photos and screenshots, on the phone's own storage. */
        @Suppress("DEPRECATION")
        fun legacyRoots(): List<String> = listOf(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DCIM).path,
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_PICTURES).path + "/Screenshots",
        )
    }
}
