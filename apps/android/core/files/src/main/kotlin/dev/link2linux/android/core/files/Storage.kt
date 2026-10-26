package dev.link2linux.android.core.files

import java.io.IOException
import java.io.InputStream

/*
 * What the engine needs from the phone: somewhere to put received files, a
 * way to read the files the user sends, and the user's choices. The Android
 * side implements them over MediaStore and the ContentResolver; tests use
 * fakes.
 */

/** Where received files go: the phone's shared Downloads. */
interface ReceivedFileStore {
    /** Bytes that can still be written before the storage is full. */
    fun freeBytes(): Long

    /**
     * Starts a file that will be called [name] (already [FileNames.safe]),
     * under a temporary name that keeps it out of sight until it is
     * [PartialFile.publish]ed.
     *
     * @param mimeType what the sender said the file is, possibly empty. It is
     *   only a hint: what the phone opens the file with follows its name.
     * @throws NoSpaceException when the storage is full.
     * @throws IOException when the file cannot be made.
     */
    fun create(name: String, mimeType: String, size: Long): PartialFile

    /**
     * Deletes the partial files an earlier run of the app left behind. The
     * engine does not keep transfers across a restart, so nothing will
     * finish them.
     */
    fun removeLeftovers()
}

/** A received file while its bytes arrive. */
interface PartialFile {
    /**
     * Appends [data].
     *
     * @throws NoSpaceException when the storage is full.
     * @throws IOException when it cannot be written.
     */
    fun append(data: ByteArray)

    /**
     * Gives the file its own name, made unique so it never replaces another
     * file, and makes it visible.
     *
     * @throws IOException when that fails; the partial file is then gone.
     */
    fun publish(): SavedFile

    /** Deletes the partial file. Does not throw. */
    fun delete()
}

/** A received file, under the name it got. [uri] opens it. */
data class SavedFile(val name: String, val uri: String, val mimeType: String)

/** The storage is full. */
class NoSpaceException(message: String) : IOException(message)

/** A file the user chose to send. */
interface OutgoingFile {
    /** Its name as the phone knows it. The engine makes it [FileNames.safe] before it goes out. */
    val name: String

    /** Its size in bytes. A file that turns out to have another size is not sent. */
    val size: Long

    /** Its MIME type, or empty when unknown. */
    val mimeType: String

    /** When it was last modified, in Unix milliseconds, or 0 when unknown. */
    val modifiedUnixMs: Long

    /**
     * Opens it at its first byte. Called again for each attempt, since a
     * resumed transfer hashes the bytes the receiver already has.
     *
     * @throws Exception of any kind when it cannot be read.
     */
    fun open(): InputStream
}

/** Whether the feature can be used right now, for `FeatureStatus` and for refusing offers. */
enum class FileAvailability {
    READY,

    /** The user turned the feature off on the phone. */
    DISABLED,

    /** Android 8 or 9, and the user has not allowed storage access (permission token `storage`). */
    PERMISSION_REQUIRED,
}

/** The user's choices, read whenever they matter. */
interface FilePolicy {
    /** The feature is turned on. */
    val enabled: Boolean

    /** The app may write to shared storage, or needs no permission for it on this Android version. */
    val storageGranted: Boolean
}
