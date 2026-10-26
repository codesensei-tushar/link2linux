package dev.link2linux.android.feature.files

import android.content.ContentResolver
import android.content.Context
import android.content.res.AssetFileDescriptor
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import dev.link2linux.android.core.files.OutgoingFile
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.InputStream
import java.util.UUID

/** A document the user shared or picked, read through the ContentResolver whenever a transfer needs it. */
internal class ContentFile(
    private val resolver: ContentResolver,
    val uri: Uri,
    override val name: String,
    override val size: Long,
    override val mimeType: String,
    override val modifiedUnixMs: Long,
) : OutgoingFile {
    override fun open(): InputStream = resolver.openInputStream(uri) ?: throw FileNotFoundException("The provider gave no stream")
}

/** A copy, in the app's cache, of a document whose provider does not tell its size. */
internal class CachedFile(
    val file: File,
    override val name: String,
    override val mimeType: String,
    override val modifiedUnixMs: Long,
) : OutgoingFile {
    override val size: Long = file.length()

    override fun open(): InputStream = FileInputStream(file)
}

/** The documents of one send, ready for the engine, and how many of the user's could not be read. */
internal class Resolved(val files: List<OutgoingFile>, val unreadable: Int)

/** Turns the URIs of a share or a pick into files the engine can send. */
internal object OutgoingFiles {

    /**
     * Looks up each document's name, size and type. A document whose size
     * its provider does not tell is copied to the cache first, so a change
     * can still be noticed. Blocks: call it off the main thread.
     *
     * Only `content:` URIs of other apps are read. A share is an intent any
     * app can send, and a `file:` URI or one of this app's own providers
     * would let it have the app send its private files.
     */
    fun resolve(context: Context, uris: List<Uri>): Resolved {
        val files = ArrayList<OutgoingFile>(uris.size)
        var unreadable = 0
        for (uri in uris) {
            val file = if (isForeignContent(context, uri)) runCatching { describe(context, uri) }.getOrNull() else null
            if (file == null) unreadable++ else files += file
        }
        return Resolved(files, unreadable)
    }

    private fun isForeignContent(context: Context, uri: Uri): Boolean {
        if (uri.scheme != ContentResolver.SCHEME_CONTENT) return false
        val authority = uri.authority ?: return false
        val provider = context.packageManager.resolveContentProvider(authority, 0) ?: return false
        return provider.packageName != context.packageName
    }

    private fun describe(context: Context, uri: Uri): OutgoingFile {
        val resolver = context.contentResolver
        var name: String? = null
        var size: Long? = null
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME).takeIf { it >= 0 && !cursor.isNull(it) }?.let { name = cursor.getString(it) }
                cursor.getColumnIndex(OpenableColumns.SIZE).takeIf { it >= 0 && !cursor.isNull(it) }?.let { size = cursor.getLong(it) }
            }
        }
        val displayName = name?.takeIf { it.isNotBlank() } ?: uri.lastPathSegment ?: "file"
        val mimeType = resolver.getType(uri) ?: Downloads.mimeTypeOf(displayName)
        val modified = lastModified(context, uri)
        val known = size?.takeIf { it >= 0 } ?: descriptorLength(resolver, uri)
        return if (known != null) {
            ContentFile(resolver, uri, displayName, known, mimeType, modified)
        } else {
            CachedFile(copyToCache(context, resolver, uri), displayName, mimeType, modified)
        }
    }

    /** Only document providers have the column, and others may refuse a query for it. */
    private fun lastModified(context: Context, uri: Uri): Long {
        if (!DocumentsContract.isDocumentUri(context, uri)) return 0
        return runCatching {
            context.contentResolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_LAST_MODIFIED), null, null, null)?.use {
                if (it.moveToFirst() && !it.isNull(0)) it.getLong(0) else 0L
            }
        }.getOrNull() ?: 0
    }

    private fun descriptorLength(resolver: ContentResolver, uri: Uri): Long? = runCatching {
        resolver.openAssetFileDescriptor(uri, "r")?.use { it.length }
    }.getOrNull()?.takeIf { it != AssetFileDescriptor.UNKNOWN_LENGTH && it >= 0 }

    private fun copyToCache(context: Context, resolver: ContentResolver, uri: Uri): File {
        val directory = File(context.cacheDir, CACHE_DIRECTORY).apply { mkdirs() }
        val copy = File(directory, UUID.randomUUID().toString())
        try {
            val input = resolver.openInputStream(uri) ?: throw FileNotFoundException("The provider gave no stream")
            input.use { source -> copy.outputStream().use { source.copyTo(it) } }
        } catch (e: Exception) {
            copy.delete()
            throw e
        }
        return copy
    }

    /** Deletes the copies an earlier run left in the cache. */
    fun clearCache(context: Context) {
        File(context.cacheDir, CACHE_DIRECTORY).listFiles()?.forEach { it.delete() }
    }

    private const val CACHE_DIRECTORY = "outgoing"
}
