package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.core.photos.PhotoAnswer
import dev.link2linux.android.core.photos.PhotoAvailability
import dev.link2linux.android.core.photos.PhotoDesktop
import dev.link2linux.android.core.photos.PhotoEngine
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.photoListChanged
import dev.link2linux.protocol.v1.photoListReply
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.IOException
import java.util.concurrent.atomic.AtomicInteger

/**
 * Connects the photos engine to sessions with paired desktops
 * (docs/protocol.md, "Photos").
 *
 * In a session where `photos` is active, a `PhotoListRequest` is answered
 * with the list and subscribes the session: a follower then sends
 * `PhotoListChanged` as the engine paces it, until the session ends, the
 * feature leaves `READY` on the phone, or the desktop's `FeatureStatus`
 * says it is not `READY` there. The desktop asks again once it is. Until it
 * asks, and after its subscription ended, it may fetch no photo.
 *
 * Thumbnail and data requests wait for the session's workers, two for
 * thumbnails and one for data, so the reader never waits. A desktop that
 * keeps more waiting than the protocol allows (8 thumbnails, 4 data
 * requests) gets `BUSY` for the rest. The answers can be hundreds of KiB, so
 * the workers send them rather than post them, and wait while they go out.
 *
 * The phone's `FeatureStatus` says `DISABLED` until the user turns photos
 * on, and `PERMISSION_REQUIRED` with `photos` until they let the app read
 * their photos. In a session where the feature is not active, nothing is
 * attached and the desktop's requests fall through to `UNSUPPORTED`.
 */
class PhotoSessionHooks(
    private val engine: PhotoEngine,
    /** Where each session's workers and follower run. */
    private val scope: CoroutineScope,
) : SessionHooks {

    private class Attached(
        val desktop: PhotoDesktop,
        /** The parent of the session's workers and follower; cancelled when it ends. */
        val job: Job,
        val thumbnails: Channel<Envelope>,
        val data: Channel<Envelope>,
        val thumbnailsInFlight: AtomicInteger,
        val dataInFlight: AtomicInteger,
    ) {
        var follower: Job? = null // guarded by the hooks' lock
    }

    private val lock = Mutex()
    private val attached = HashMap<Session, Attached>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.PHOTOS)

    override fun featureStatus(): FeatureStatus = statusOf(engine.availability.value)

    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.availabilityChanges.map(::statusOf)

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        if (!isActive(session)) return
        val job = SupervisorJob(scope.coroutineContext[Job])
        val ours = Attached(
            PhotoDesktop(),
            job,
            Channel(MAX_THUMBNAILS_WAITING),
            Channel(MAX_DATA_WAITING),
            AtomicInteger(),
            AtomicInteger(),
        )
        repeat(THUMBNAIL_WORKERS) { scope.launch(job) { for (request in ours.thumbnails) answer(ours, session, request) } }
        repeat(DATA_WORKERS) { scope.launch(job) { for (request in ours.data) answer(ours, session, request) } }
        lock.withLock { attached[session] = ours }
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!isActive(session)) return false
        when (envelope.payloadCase) {
            PayloadCase.PHOTO_LIST_REQUEST, PayloadCase.PHOTO_THUMBNAIL_REQUEST, PayloadCase.PHOTO_DATA_REQUEST -> Unit
            PayloadCase.FEATURE_STATUS -> {
                if (!session.isReadyOnPeer(Feature.PHOTOS.wireName)) stop(session)
                return true
            }
            else -> return false
        }
        // Only a request is answered; one sent as a one-way event is ignored.
        if (!envelope.expectsReply) return true
        // Attached before the reader starts, and only removed once it stopped.
        val ours = lock.withLock { attached[session] } ?: return true
        when (envelope.payloadCase) {
            PayloadCase.PHOTO_LIST_REQUEST -> follow(ours, session, envelope.seq)
            PayloadCase.PHOTO_THUMBNAIL_REQUEST ->
                queue(ours.thumbnails, ours.thumbnailsInFlight, MAX_THUMBNAILS_WAITING, session, envelope)
            else -> queue(ours.data, ours.dataInFlight, MAX_DATA_WAITING, session, envelope)
        }
        return true
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        val ended = lock.withLock { attached.remove(session) } ?: return
        ended.thumbnails.close()
        ended.data.close()
        ended.job.cancel()
        ended.desktop.forget()
    }

    /** Answers the list request [seq] of [session], then keeps it current, replacing an earlier follower. */
    private suspend fun follow(ours: Attached, session: Session, seq: Long) {
        val follower = scope.launch(ours.job, start = CoroutineStart.LAZY) {
            try {
                engine.subscribe(
                    desktop = ours.desktop,
                    reply = { answer ->
                        session.send {
                            replyTo = seq
                            when (answer) {
                                is PhotoAnswer.Listed -> photoListReply = photoListReply { photos += answer.photos }
                                is PhotoAnswer.Refused -> errorReply = answer.message
                                else -> error("A list request answered with $answer")
                            }
                        }
                    },
                    changed = { list -> session.send { photoListChanged = photoListChanged { photos += list } } },
                )
            } catch (_: IOException) {
                // The connection failed; the session ends with it.
            }
        }
        lock.withLock { ours.follower.also { ours.follower = follower } }?.cancel()
        follower.start()
    }

    /** The desktop's photos are not ready there: its subscription ends, and with it what it may fetch. */
    private suspend fun stop(session: Session) {
        val ours = lock.withLock { attached[session] } ?: return
        lock.withLock { ours.follower.also { ours.follower = null } }?.cancel()
        ours.desktop.forget()
    }

    /**
     * Hands [envelope] to a worker, or answers `BUSY` when the desktop already
     * has [max] without an answer. Requests a worker has taken still count:
     * the queue alone does not, since it no longer holds them.
     */
    private fun queue(requests: Channel<Envelope>, inFlight: AtomicInteger, max: Int, session: Session, envelope: Envelope) {
        if (inFlight.incrementAndGet() > max || requests.trySend(envelope).isFailure) {
            inFlight.decrementAndGet()
            // Posted: the reader never waits for a write.
            session.post {
                replyTo = envelope.seq
                errorReply = errorReply {
                    code = ErrorCode.ERROR_CODE_BUSY
                    detail = "Too many photo requests wait for their answers"
                    retryAfterMs = PhotoEngine.BUSY_RETRY_MILLIS.toInt()
                }
            }
        }
    }

    /** Answers a thumbnail or data request; runs on one of the session's workers. */
    private suspend fun answer(ours: Attached, session: Session, envelope: Envelope) {
        val inFlight = if (envelope.payloadCase == PayloadCase.PHOTO_THUMBNAIL_REQUEST) {
            ours.thumbnailsInFlight
        } else {
            ours.dataInFlight
        }
        try {
            val answer = when (envelope.payloadCase) {
                PayloadCase.PHOTO_THUMBNAIL_REQUEST -> engine.thumbnail(ours.desktop, envelope.photoThumbnailRequest)
                else -> engine.data(ours.desktop, envelope.photoDataRequest)
            }
            session.send {
                replyTo = envelope.seq
                when (answer) {
                    is PhotoAnswer.Thumbnail -> photoThumbnail = answer.message
                    is PhotoAnswer.Data -> photoData = answer.message
                    is PhotoAnswer.Refused -> errorReply = answer.message
                    is PhotoAnswer.Listed -> error("A photo request answered with a list")
                }
            }
        } catch (_: IOException) {
            // The connection failed; the session ends with it, and the workers with the session.
        } finally {
            inFlight.decrementAndGet()
        }
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.PHOTOS.wireName)

    private fun statusOf(availability: PhotoAvailability): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.PHOTOS.wireName
            when (availability) {
                PhotoAvailability.READY -> this.availability = FeatureAvailability.FEATURE_AVAILABILITY_READY
                PhotoAvailability.DISABLED -> this.availability = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
                PhotoAvailability.PERMISSION_REQUIRED -> {
                    this.availability = FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED
                    permission = PhotoEngine.PERMISSION
                }
            }
        }
    }

    internal companion object {
        /** Thumbnail requests a desktop may have without an answer in one session. */
        const val MAX_THUMBNAILS_WAITING = 8

        /** Data requests a desktop may have without an answer in one session. */
        const val MAX_DATA_WAITING = 4

        /** Thumbnails of one session made at once, so a large preview does not hold up the grid. */
        const val THUMBNAIL_WORKERS = 2

        /** One photo's bytes at a time per session: a read is up to 256 KiB and the desktop sends few. */
        const val DATA_WORKERS = 1
    }
}
