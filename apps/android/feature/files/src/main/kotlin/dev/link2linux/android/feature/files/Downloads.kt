package dev.link2linux.android.feature.files

import android.annotation.SuppressLint
import android.content.ContentResolver
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.StatFs
import android.os.storage.StorageManager
import android.provider.MediaStore
import android.system.ErrnoException
import android.system.OsConstants
import android.webkit.MimeTypeMap
import androidx.annotation.RequiresApi
import dev.link2linux.android.core.files.FileNames
import dev.link2linux.android.core.files.NoSpaceException
import dev.link2linux.android.core.files.PartialFile
import dev.link2linux.android.core.files.ReceivedFileStore
import dev.link2linux.android.core.files.SavedFile
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.OutputStream
import java.util.Locale
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** The phone's shared Downloads, where received files go. */
internal object Downloads {

    fun store(context: Context): ReceivedFileStore =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) MediaStoreDownloads(context) else PublicDownloadDirectory(context)

    /**
     * The MIME type Android knows for [name]'s extension. A received file
     * is typed by its name, never by what the sender said: that decides
     * which app opens it.
     */
    fun mimeTypeOf(name: String): String {
        val extension = name.substringAfterLast('.', "").lowercase(Locale.ROOT)
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: OCTET_STREAM
    }

    /**
     * Bytes that may be written to shared storage now. Android's allocatable
     * bytes leave the system's reserve but also count other apps' caches,
     * which only an app that asks for them to be cleared gets back, so the
     * space free right now caps them: the receiver's spare must stay free.
     */
    @SuppressLint("UsableSpace")
    fun freeBytes(context: Context): Long {
        val directory = context.getExternalFilesDir(null) ?: throw IOException("Shared storage is not mounted")
        val storage = context.getSystemService(StorageManager::class.java)
        return minOf(storage.getAllocatableBytes(storage.getUuidForPath(directory)), directory.usableSpace)
    }

    /** Runs [block], turning what storage throws into the engine's exceptions. */
    inline fun <T> guarded(block: () -> T): T = try {
        block()
    } catch (e: IOException) {
        throw if (isNoSpace(e)) NoSpaceException("Shared storage is full") else e
    } catch (e: RuntimeException) {
        // MediaStore reports refusals as IllegalStateException, IllegalArgumentException or SecurityException.
        throw IOException(e)
    }

    fun isNoSpace(e: Throwable): Boolean = generateSequence(e) { it.cause }.any {
        (it is ErrnoException && it.errno == OsConstants.ENOSPC) || it.message?.contains("ENOSPC") == true
    }

    const val OCTET_STREAM = "application/octet-stream"
}

/**
 * Android 10 and later: files are added to `MediaStore.Downloads` with
 * `IS_PENDING` set, which keeps them out of other apps' sight until they
 * are complete. MediaStore picks a free name when the one asked for is
 * taken, so a received file never replaces another.
 */
@RequiresApi(Build.VERSION_CODES.Q)
private class MediaStoreDownloads(private val context: Context) : ReceivedFileStore {
    private val resolver: ContentResolver = context.contentResolver
    private val collection: Uri = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

    override fun freeBytes(): Long = Downloads.freeBytes(context)

    override fun create(name: String, mimeType: String, size: Long): PartialFile {
        val type = Downloads.mimeTypeOf(name)
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, type)
            put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = Downloads.guarded { resolver.insert(collection, values) } ?: throw IOException("MediaStore did not add the file")
        val output = try {
            Downloads.guarded { resolver.openOutputStream(uri, "w") } ?: throw IOException("MediaStore gave no stream")
        } catch (e: IOException) {
            runCatching { resolver.delete(uri, null, null) }
            throw e
        }
        return Entry(uri, name, type, output)
    }

    private inner class Entry(val uri: Uri, val name: String, val type: String, val output: OutputStream) : PartialFile {
        override fun append(data: ByteArray) = Downloads.guarded { output.write(data) }

        override fun publish(): SavedFile {
            try {
                Downloads.guarded {
                    output.close()
                    resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
                }
            } catch (e: IOException) {
                delete()
                throw e
            }
            // The name it got, which differs from the one asked for when that was taken.
            val given = runCatching {
                resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                    if (cursor.moveToFirst()) cursor.getString(0) else null
                }
            }.getOrNull()
            return SavedFile(given ?: name, uri.toString(), type)
        }

        override fun delete() {
            runCatching { output.close() }
            runCatching { resolver.delete(uri, null, null) }
        }
    }

    /** Pending files are this app's own: MediaStore shows no other app's. */
    override fun removeLeftovers() {
        val projection = arrayOf(MediaStore.MediaColumns._ID)
        val selection = "${MediaStore.MediaColumns.OWNER_PACKAGE_NAME} = ?"
        val args = arrayOf(context.packageName)
        val cursor = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val query = Bundle().apply {
                putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_ONLY)
                putString(ContentResolver.QUERY_ARG_SQL_SELECTION, selection)
                putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, args)
            }
            resolver.query(collection, projection, query, null)
        } else {
            @Suppress("DEPRECATION")
            resolver.query(
                MediaStore.setIncludePending(collection),
                projection,
                "${MediaStore.MediaColumns.IS_PENDING} = 1 AND $selection",
                args,
                null,
            )
        }
        cursor?.use {
            while (it.moveToNext()) {
                runCatching { resolver.delete(ContentUris.withAppendedId(collection, it.getLong(0)), null, null) }
            }
        }
    }
}

/**
 * Android 8 and 9: files are written to the public Download directory,
 * which needs WRITE_EXTERNAL_STORAGE. Each file is written under a hidden
 * temporary name; once complete, a free name is claimed by creating it,
 * which fails when the name is taken, and the file is renamed onto it.
 */
private class PublicDownloadDirectory(private val context: Context) : ReceivedFileStore {
    @Suppress("DEPRECATION")
    private val directory: File get() = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)

    @Suppress("DEPRECATION")
    override fun freeBytes(): Long = StatFs(Environment.getExternalStorageDirectory().path).availableBytes

    override fun create(name: String, mimeType: String, size: Long): PartialFile {
        val directory = directory
        val temp = File(directory, "$TEMP_PREFIX${UUID.randomUUID()}$TEMP_SUFFIX")
        val output = Downloads.guarded {
            if (!directory.isDirectory && !directory.mkdirs()) throw IOException("The Download directory cannot be made")
            FileOutputStream(temp)
        }
        return Entry(directory, temp, name, output)
    }

    private inner class Entry(val directory: File, val temp: File, val name: String, val output: FileOutputStream) : PartialFile {
        override fun append(data: ByteArray) = Downloads.guarded { output.write(data) }

        override fun publish(): SavedFile {
            val target = try {
                Downloads.guarded {
                    output.close()
                    val target = claim()
                    if (!temp.renameTo(target)) {
                        target.delete()
                        throw IOException("The file could not be renamed")
                    }
                    target
                }
            } catch (e: IOException) {
                delete()
                throw e
            }
            val type = Downloads.mimeTypeOf(target.name)
            return SavedFile(target.name, scan(target, type)?.toString().orEmpty(), type)
        }

        /** Creates the first free name in the series `name`, `name (1)`, ... and returns it. */
        private fun claim(): File {
            for (n in 0..MAX_NUMBER) {
                val candidate = File(directory, if (n == 0) name else FileNames.numbered(name, n))
                if (candidate.createNewFile()) return candidate
            }
            throw IOException("No free name for the file")
        }

        override fun delete() {
            runCatching { output.close() }
            temp.delete()
        }
    }

    /** Adds [file] to MediaStore, so it shows in Downloads, and returns the content URI it got. */
    private fun scan(file: File, type: String): Uri? {
        val done = CountDownLatch(1)
        var result: Uri? = null
        MediaScannerConnection.scanFile(context, arrayOf(file.path), arrayOf(type)) { _, uri ->
            result = uri
            done.countDown()
        }
        done.await(SCAN_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        return result
    }

    override fun removeLeftovers() {
        directory.listFiles { file -> file.name.startsWith(TEMP_PREFIX) && file.name.endsWith(TEMP_SUFFIX) }
            ?.forEach { it.delete() }
    }

    private companion object {
        const val TEMP_PREFIX = ".link2linux-"
        const val TEMP_SUFFIX = ".partial"
        const val MAX_NUMBER = 999
        const val SCAN_TIMEOUT_SECONDS = 10L
    }
}
