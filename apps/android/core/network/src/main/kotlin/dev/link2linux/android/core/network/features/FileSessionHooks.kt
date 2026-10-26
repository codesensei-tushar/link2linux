package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.files.AnswerTimeoutException
import dev.link2linux.android.core.files.FileAnswer
import dev.link2linux.android.core.files.FileAvailability
import dev.link2linux.android.core.files.FileEngine
import dev.link2linux.android.core.files.FileLink
import dev.link2linux.android.core.files.FilePeer
import dev.link2linux.android.core.files.FileRequest
import dev.link2linux.android.core.files.PendingAnswer
import dev.link2linux.android.core.files.toFilePeer
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.RequestTimeoutException
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureStatus
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Connects the files engine to sessions with paired desktops.
 *
 * The feature is active in a session when both Hellos list `files`; in a
 * session where it is not, nothing is attached and the desktop's file
 * requests fall through to `ERROR_CODE_UNSUPPORTED`. In a session where it
 * is, the session is the engine's link to that desktop: the engine answers
 * the desktop's offers, chunks and cancels, and sends its own through
 * [Session.sendRequest], whose answers the session hands back. The phone's
 * `FeatureStatus` says `DISABLED` until the user turns files on, and
 * `PERMISSION_REQUIRED` with `storage` on Android 8 and 9 until they allow
 * storage access.
 *
 * The desktop's requests are answered by a worker per session, in the order
 * they came: a chunk is written to storage before its `Ack`, and the
 * session's reader, which must never wait, only queues them.
 */
class FileSessionHooks(
    private val engine: FileEngine,
    /** Where each session's worker runs. */
    private val scope: CoroutineScope,
    /** How long a request of the phone waits for the desktop's answer. */
    private val requestTimeoutMillis: Long = Session.REQUEST_TIMEOUT_MILLIS,
) : SessionHooks {

    private class Attached(val link: SessionLink, val requests: Channel<Envelope>, val worker: Job)

    private val lock = Mutex()
    private val attached = HashMap<Session, Attached>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.FILES)

    override fun featureStatus(): FeatureStatus = statusOf(engine.availability.value)

    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.availabilityChanges.map(::statusOf)

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        if (!isActive(session)) return
        val link = SessionLink(desktop.toFilePeer(), session, requestTimeoutMillis)
        val requests = Channel<Envelope>(MAX_WAITING)
        val worker = scope.launch {
            for (envelope in requests) answer(link, session, envelope)
        }
        lock.withLock { attached[session] = Attached(link, requests, worker) }
        engine.attach(link)
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!isActive(session)) return false
        when (envelope.payloadCase) {
            PayloadCase.FILE_OFFER, PayloadCase.FILE_CHUNK, PayloadCase.FILE_CANCEL -> Unit
            else -> return false
        }
        // Only a request is answered; a request message sent as a one-way event is ignored.
        if (!envelope.expectsReply) return true
        // Attached before the reader starts, and only removed once it stopped.
        val requests = lock.withLock { attached[session] }?.requests ?: return true
        // At most 8 chunks wait for their answers; far more waiting is a peer that ignores that.
        if (requests.trySend(envelope).isFailure) session.protocolError("More than $MAX_WAITING file requests wait for their answers")
        return true
    }

    /** Answers one of the desktop's requests; runs on the session's worker. */
    private suspend fun answer(link: SessionLink, session: Session, envelope: Envelope) {
        val request = when (envelope.payloadCase) {
            PayloadCase.FILE_OFFER -> FileRequest.Offer(envelope.fileOffer)
            PayloadCase.FILE_CHUNK -> FileRequest.Chunk(envelope.fileChunk)
            else -> FileRequest.Cancel(envelope.fileCancel)
        }
        val answer = try {
            engine.handle(link, request)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // The worker answers what comes after it all the same.
            FileAnswer.Error(
                errorReply {
                    code = ErrorCode.ERROR_CODE_INTERNAL
                    detail = "The phone failed: ${e.javaClass.simpleName}"
                },
            )
        }
        session.post {
            replyTo = envelope.seq
            when (answer) {
                is FileAnswer.Accept -> fileAccept = answer.message
                FileAnswer.Ack -> ack = Ack.getDefaultInstance()
                is FileAnswer.Error -> errorReply = answer.message
            }
        }
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        val ended = lock.withLock { attached.remove(session) } ?: return
        ended.requests.close()
        // What the worker still does must not reach the engine after the session left it.
        ended.worker.cancelAndJoin()
        engine.detach(ended.link)
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.FILES.wireName)

    private fun statusOf(availability: FileAvailability): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.FILES.wireName
            when (availability) {
                FileAvailability.READY -> this.availability = FeatureAvailability.FEATURE_AVAILABILITY_READY
                FileAvailability.DISABLED -> this.availability = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
                FileAvailability.PERMISSION_REQUIRED -> {
                    this.availability = FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED
                    permission = FileEngine.STORAGE_PERMISSION
                }
            }
        }
    }

    private companion object {
        /**
         * The desktop's requests that may wait for the worker: 8 chunks, and offers and cancels besides.
         * Pipelined requests have no other bound, so this matches the session's posted-answer cap.
         */
        const val MAX_WAITING = Session.MAX_POSTED
    }

    /** One session, as the engine's link to its desktop. */
    private class SessionLink(
        override val peer: FilePeer,
        private val session: Session,
        private val timeoutMillis: Long,
    ) : FileLink {

        override val peerReady: Flow<Boolean> = session.peerFeatures
            .map { it[Feature.FILES.wireName] == FeatureAvailability.FEATURE_AVAILABILITY_READY }
            .distinctUntilChanged()

        override suspend fun send(request: FileRequest): PendingAnswer {
            val pending = session.sendRequest(timeoutMillis) {
                when (request) {
                    is FileRequest.Offer -> fileOffer = request.message
                    is FileRequest.Chunk -> fileChunk = request.message
                    is FileRequest.Cancel -> fileCancel = request.message
                }
            }
            return PendingAnswer {
                val answer = try {
                    pending.await()
                } catch (e: RequestTimeoutException) {
                    throw AnswerTimeoutException(e.message.orEmpty())
                }
                when {
                    answer.hasErrorReply() -> FileAnswer.Error(answer.errorReply)
                    request is FileRequest.Offer && answer.hasFileAccept() -> FileAnswer.Accept(answer.fileAccept)
                    request !is FileRequest.Offer && answer.hasAck() -> FileAnswer.Ack
                    // The protocol allows no other answer.
                    else -> session.protocolError("A ${request::class.simpleName} answered with ${answer.payloadCase}")
                }
            }
        }
    }
}
