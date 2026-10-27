package dev.link2linux.android.core.photos

import com.google.protobuf.ByteString
import dev.link2linux.android.core.files.FileNames
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ErrorReply
import dev.link2linux.protocol.v1.ImageFormat
import dev.link2linux.protocol.v1.Photo
import dev.link2linux.protocol.v1.PhotoData
import dev.link2linux.protocol.v1.PhotoDataRequest
import dev.link2linux.protocol.v1.PhotoThumbnail
import dev.link2linux.protocol.v1.PhotoThumbnailRequest
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.photoData
import dev.link2linux.protocol.v1.photoThumbnail
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.retryWhen
import kotlinx.coroutines.flow.shareIn
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import java.util.concurrent.atomic.AtomicInteger

/** The engine's answer to one of a desktop's requests. */
sealed interface PhotoAnswer {
    /** The list for a `PhotoListReply`; the desktop is now subscribed to it. */
    data class Listed(val photos: List<Photo>) : PhotoAnswer

    data class Thumbnail(val message: PhotoThumbnail) : PhotoAnswer

    data class Data(val message: PhotoData) : PhotoAnswer

    data class Refused(val message: ErrorReply) : PhotoAnswer
}

/**
 * One desktop's subscription in one session: the list it was last sent,
 * which is all it may fetch (docs/protocol.md, "Photos", and implementation
 * requirement 13). Nothing until it asks for the list, and nothing again
 * once its subscription ends, until it asks again.
 */
class PhotoDesktop {
    /** A list as it went to the desktop. Compared by identity, so each list sent is its own. */
    internal class Sent(val photos: List<Photo>) {
        val byId: Map<String, Photo> = photos.associateBy(Photo::getId)
    }

    private var sent: Sent? = null // guarded by this

    internal fun current(): Sent? = synchronized(this) { sent }

    internal fun start(photos: List<Photo>): Sent = synchronized(this) { Sent(photos).also { sent = it } }

    /** Replaces [old] with [new] if [old] is still the list the desktop has; false when it asked again since. */
    internal fun replace(old: Sent, new: Sent?): Boolean = synchronized(this) {
        if (sent !== old) return false
        sent = new
        true
    }

    /** The listed photo named [id], or null when the desktop was not sent one. */
    internal fun find(id: String): Photo? = current()?.byId?.get(id)

    /** The desktop's subscription ended: it may fetch nothing until it asks for the list again. */
    fun forget() = synchronized(this) { sent = null }
}

/**
 * The phone's side of the photos feature (docs/protocol.md, "Photos").
 *
 * A desktop asks for the list with [list], which subscribes it, and
 * [follow] then keeps it current with `PhotoListChanged`. It fetches
 * thumbnails with [thumbnail] and the original files with [data], and only
 * for the photos of the list it was last sent: any other id is
 * `NOT_FOUND`, as is a photo deleted since. Each [PhotoDesktop] holds that
 * list for one desktop in one session.
 *
 * The engine listens to the platform for changes only while a desktop
 * follows, and for [keepListeningMillis] after the last one stops. A list
 * that fails to load is loaded again after [retryMillis], and [onFailure]
 * hears of every failure; it must not log what the failure carries.
 *
 * Decoding and reading run a few at a time across all desktops; past
 * [MAX_THUMBNAILS_WAITING] thumbnails or [MAX_DATA_WAITING] reads waiting,
 * a request is answered `BUSY` with [busyRetryMillis].
 */
class PhotoEngine(
    scope: CoroutineScope,
    private val library: PhotoLibrary,
    /** Whether the photos can be shared now: the user's switch and the permission. */
    val availability: StateFlow<PhotoAvailability>,
    keepListeningMillis: Long = KEEP_LISTENING_MILLIS,
    private val intervalMillis: Long = CHANGE_INTERVAL_MILLIS,
    retryMillis: Long = RETRY_MILLIS,
    private val busyRetryMillis: Long = BUSY_RETRY_MILLIS,
    private val onFailure: (Throwable) -> Unit = {},
) {
    private val refreshes = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    // The replay ends with the listener: a list from before then may be stale.
    private val lists: SharedFlow<List<Photo>> = merge(library.changes(), refreshes)
        // A burst of changes while a list loads needs one more load, not one each.
        .conflate()
        .map { checked(library.photos(MAX_PHOTOS)) }
        .retryWhen { cause, _ ->
            onFailure(cause)
            delay(retryMillis)
            true
        }
        .distinctUntilChanged()
        .shareIn(scope, SharingStarted.WhileSubscribed(keepListeningMillis, replayExpirationMillis = 0), replay = 1)

    private val decoding = Semaphore(DECODING_AT_ONCE)
    private val reading = Semaphore(READING_AT_ONCE)
    private val thumbnailsWaiting = AtomicInteger()
    private val dataWaiting = AtomicInteger()

    /**
     * Loads the list again for the desktops that follow it, for a change the
     * platform does not report, such as the user picking other photos for
     * the app to see.
     */
    fun refresh() {
        refreshes.tryEmit(Unit)
    }

    /**
     * Answers [desktop]'s `PhotoListRequest`: the current list, which it may
     * fetch from from now on, or why not.
     */
    suspend fun list(desktop: PhotoDesktop): PhotoAnswer {
        refusal()?.let { return it }
        val photos = try {
            checked(library.photos(MAX_PHOTOS))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            onFailure(e)
            return refused(ErrorCode.ERROR_CODE_INTERNAL, "The phone could not list its photos: ${e.javaClass.simpleName}")
        }
        desktop.start(photos)
        return PhotoAnswer.Listed(photos)
    }

    /**
     * Keeps [desktop] current after its [list]: [send] gets each new list,
     * at most one every [intervalMillis] counted from the last one the
     * desktop got, starting with the reply. A change in the meantime
     * replaces the one waiting, so only the newest goes out, and nothing goes
     * out when the list ends up as the desktop has it.
     *
     * Returns when the feature leaves `READY` or the desktop asks for the
     * list again, and ends the subscription then, as when cancelled: the
     * desktop may fetch nothing until it asks again. Throws what [send]
     * throws.
     */
    suspend fun follow(desktop: PhotoDesktop, send: suspend (List<Photo>) -> Unit) {
        var sent = desktop.current() ?: return
        try {
            coroutineScope {
                // Subscribed for as long as the desktop follows, so the listener stays on between updates.
                val latest = lists.stateIn(this, SharingStarted.Eagerly, null)
                val updates = launch {
                    while (true) {
                        delay(intervalMillis)
                        val next = latest.filterNotNull().first { it != sent.photos }
                        val update = PhotoDesktop.Sent(next)
                        // Before sending: the desktop may fetch a new photo as soon as it sees it.
                        if (!desktop.replace(sent, update)) break
                        sent = update
                        send(next)
                    }
                }
                val leftReady = launch { availability.first { it != PhotoAvailability.READY } }
                select {
                    updates.onJoin {}
                    leftReady.onJoin {}
                }
                coroutineContext.cancelChildren()
            }
        } finally {
            desktop.replace(sent, null)
        }
    }

    /** Answers [desktop]'s `PhotoThumbnailRequest`. */
    suspend fun thumbnail(desktop: PhotoDesktop, request: PhotoThumbnailRequest): PhotoAnswer {
        refusal()?.let { return it }
        // A uint32 past Int.MAX_VALUE arrives negative.
        if (request.maxSize !in MIN_THUMBNAIL_SIZE..MAX_THUMBNAIL_SIZE) {
            return refused(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, "max_size must be $MIN_THUMBNAIL_SIZE to $MAX_THUMBNAIL_SIZE")
        }
        val photo = desktop.find(request.id) ?: return notFound()
        return whenNotBusy(thumbnailsWaiting, MAX_THUMBNAILS_WAITING) {
            decoding.withPermit {
                val decoded = library.decode(photo, request.maxSize) ?: return@withPermit notFound()
                decoded.use { PhotoAnswer.Thumbnail(fit(it, request.maxSize)) }
            }
        }
    }

    /** Answers [desktop]'s `PhotoDataRequest`. */
    suspend fun data(desktop: PhotoDesktop, request: PhotoDataRequest): PhotoAnswer {
        refusal()?.let { return it }
        val photo = desktop.find(request.id) ?: return notFound()
        // A uint64 past Long.MAX_VALUE arrives negative, and is past the end of any file.
        if (request.offset < 0) return pastTheEnd()
        return whenNotBusy(dataWaiting, MAX_DATA_WAITING) {
            reading.withPermit {
                val bytes = library.read(photo, request.offset, MAX_DATA_BYTES) ?: return@withPermit notFound()
                when {
                    request.offset > bytes.size -> pastTheEnd()
                    bytes.data.size > MAX_DATA_BYTES -> error("The platform read ${bytes.data.size} bytes")
                    else -> PhotoAnswer.Data(
                        photoData {
                            size = bytes.size
                            modifiedUnixMs = bytes.modifiedUnixMs
                            data = ByteString.copyFrom(bytes.data)
                        },
                    )
                }
            }
        }
    }

    /**
     * Runs [answer] unless [waiting] already counts [max] requests, which
     * gets `BUSY`. A failure of the platform gets `INTERNAL`.
     */
    private suspend fun whenNotBusy(waiting: AtomicInteger, max: Int, answer: suspend () -> PhotoAnswer): PhotoAnswer {
        if (waiting.incrementAndGet() > max) {
            waiting.decrementAndGet()
            return PhotoAnswer.Refused(
                errorReply {
                    code = ErrorCode.ERROR_CODE_BUSY
                    detail = "The phone has too many photo requests"
                    retryAfterMs = busyRetryMillis.toInt()
                },
            )
        }
        return try {
            answer()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            onFailure(e)
            refused(ErrorCode.ERROR_CODE_INTERNAL, "The phone could not read the photo: ${e.javaClass.simpleName}")
        } finally {
            waiting.decrementAndGet()
        }
    }

    /**
     * A JPEG of [photo] that fits [MAX_THUMBNAIL_BYTES]: at the size asked
     * for, or the photo's own when smaller, at a lower quality and then a
     * smaller size until it fits.
     */
    private fun fit(photo: ScaledPhoto, maxSize: Int): PhotoThumbnail {
        var side = minOf(maxSize, maxOf(photo.width, photo.height)).coerceAtLeast(1)
        while (true) {
            for (quality in QUALITIES) {
                val image = photo.jpeg(side, quality)
                if (image.data.size <= MAX_THUMBNAIL_BYTES) {
                    return photoThumbnail {
                        format = ImageFormat.IMAGE_FORMAT_JPEG
                        data = ByteString.copyFrom(image.data)
                        width = image.width
                        height = image.height
                    }
                }
            }
            // A small JPEG always fits; one that does not means the platform encodes something else.
            check(side > MIN_THUMBNAIL_SIZE) { "A JPEG of ${side}px is over $MAX_THUMBNAIL_BYTES bytes" }
            side = maxOf(MIN_THUMBNAIL_SIZE, side * 3 / 4)
        }
    }

    /** The answer every request gets while the feature is not `READY`, or null when it is. */
    private fun refusal(): PhotoAnswer.Refused? = when (availability.value) {
        PhotoAvailability.READY -> null
        PhotoAvailability.DISABLED -> refused(ErrorCode.ERROR_CODE_DISABLED, "Photos are turned off on the phone")
        PhotoAvailability.PERMISSION_REQUIRED -> PhotoAnswer.Refused(
            errorReply {
                code = ErrorCode.ERROR_CODE_PERMISSION_REQUIRED
                detail = "The phone may not read its photos"
                permission = PERMISSION
            },
        )
    }

    private fun notFound() = refused(ErrorCode.ERROR_CODE_NOT_FOUND, "No such photo in the list")

    private fun pastTheEnd() = refused(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, "The offset is past the end of the file")

    private fun refused(code: ErrorCode, detail: String) = PhotoAnswer.Refused(
        errorReply {
            this.code = code
            this.detail = detail
        },
    )

    companion object {
        /** The permission token for reading the phone's photos. */
        const val PERMISSION = "photos"

        /** The most photos in a list (docs/protocol.md, "Photos"). */
        const val MAX_PHOTOS = 200

        /** The longest `Photo.id`, in bytes of UTF-8. */
        const val MAX_ID_BYTES = 64

        const val MIN_THUMBNAIL_SIZE = 64
        const val MAX_THUMBNAIL_SIZE = 2048

        /** The largest `PhotoThumbnail.data`. */
        const val MAX_THUMBNAIL_BYTES = 400 * 1024

        /** The bytes of a photo one `PhotoData` carries. */
        const val MAX_DATA_BYTES = 256 * 1024

        /** The shortest time between two `PhotoListChanged` to one desktop. */
        const val CHANGE_INTERVAL_MILLIS: Long = 5_000

        const val KEEP_LISTENING_MILLIS: Long = 5_000

        const val RETRY_MILLIS: Long = 2_000

        const val BUSY_RETRY_MILLIS: Long = 1_000

        /** Thumbnails decoded at once: a large one holds a bitmap of up to 16 MB. */
        const val DECODING_AT_ONCE = 2

        const val READING_AT_ONCE = 2

        /** Thumbnail requests running or waiting, across desktops: each may have 8 unanswered. */
        const val MAX_THUMBNAILS_WAITING = 16

        /** Data requests running or waiting, across desktops: each may have 4 unanswered. */
        const val MAX_DATA_WAITING = 8

        /** The JPEG qualities tried at each size, best first. */
        private val QUALITIES = intArrayOf(85, 70, 55, 40)

        /**
         * [photos] as the protocol allows them: ids of 1 to 64 bytes, each
         * once; names made safe as received file names are; no negative
         * sizes; newest first, at most [MAX_PHOTOS].
         */
        internal fun checked(photos: List<Photo>): List<Photo> {
            val seen = HashSet<String>()
            return photos.asSequence()
                .filter { it.id.isNotEmpty() && FileNames.utf8Length(it.id) <= MAX_ID_BYTES && seen.add(it.id) }
                .map { photo ->
                    photo.toBuilder().apply {
                        name = FileNames.safe(name, fallback = NAME_FALLBACK)
                        if (size < 0) size = 0
                        if (width < 0 || height < 0) {
                            width = 0
                            height = 0
                        }
                    }.build()
                }
                .sortedByDescending(Photo::getTakenUnixMs)
                .take(MAX_PHOTOS)
                .toList()
        }

        private const val NAME_FALLBACK = "photo"
    }
}
