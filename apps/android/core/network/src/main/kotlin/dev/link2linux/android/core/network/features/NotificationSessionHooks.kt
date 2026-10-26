package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.core.notifications.NotificationAvailability
import dev.link2linux.android.core.notifications.NotificationEngine
import dev.link2linux.android.core.notifications.NotificationEvent
import dev.link2linux.android.core.notifications.NotificationReply
import dev.link2linux.android.core.notifications.NotificationRequest
import dev.link2linux.android.core.notifications.NotificationTransport
import dev.link2linux.android.core.notifications.ReplySink
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Connects the notifications engine to sessions with paired desktops.
 *
 * The feature is active in a session when the desktop's `Hello` lists
 * `notifications`; in a session where it is not, nothing is attached and
 * its requests fall through to `ERROR_CODE_UNSUPPORTED`.
 *
 * Every session with the feature is attached to the engine, so with several
 * desktops connected each gets the live events once it has synced. The
 * session sends a new `FeatureStatus` whenever the engine's availability
 * changes.
 */
class NotificationSessionHooks(
    private val engine: NotificationEngine,
    private val scope: CoroutineScope,
) : SessionHooks {

    private val lock = Mutex()
    private val attached = HashMap<Session, NotificationTransport>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.NOTIFICATIONS)

    override fun featureStatus(): FeatureStatus = statusOf(engine.availability.value)

    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.availabilityChanges.map(::statusOf)

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        if (!isActive(session)) return
        val transport = SessionTransport(session, scope)
        lock.withLock {
            attached.put(session, transport)?.let { engine.detach(it) }
            engine.attach(transport)
        }
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!isActive(session)) return false
        val request = when (envelope.payloadCase) {
            PayloadCase.NOTIFICATION_SYNC_REQUEST -> NotificationRequest.Sync(envelope.notificationSyncRequest)
            PayloadCase.NOTIFICATION_DISMISS_REQUEST -> NotificationRequest.Dismiss(envelope.notificationDismissRequest)
            PayloadCase.NOTIFICATION_ACTION_REQUEST -> NotificationRequest.Action(envelope.notificationActionRequest)
            PayloadCase.IMAGE_REQUEST -> NotificationRequest.Image(envelope.imageRequest)
            else -> return false
        }
        // Only a request is answered; a request message sent as a one-way event is ignored.
        if (!envelope.expectsReply) return true
        // A session that already ended still gets its answer, but no live events.
        val from = lock.withLock { attached[session] } ?: SessionTransport(session, scope)
        engine.handle(request, RepliesTo(session, envelope.seq), from)
        return true
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        lock.withLock { attached.remove(session)?.let { engine.detach(it) } }
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.NOTIFICATIONS.wireName)

    private fun statusOf(availability: NotificationAvailability): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.NOTIFICATIONS.wireName
            when (availability) {
                NotificationAvailability.READY -> this.availability = FeatureAvailability.FEATURE_AVAILABILITY_READY
                NotificationAvailability.DISABLED -> this.availability = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
                NotificationAvailability.PERMISSION_REQUIRED -> {
                    this.availability = FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED
                    permission = NotificationEngine.NOTIFICATION_ACCESS
                }
            }
        }
    }

    private class SessionTransport(private val session: Session, private val scope: CoroutineScope) : NotificationTransport {
        override suspend fun send(event: NotificationEvent) {
            when (event) {
                is NotificationEvent.Posted -> session.send { notificationPosted = event.message }
                is NotificationEvent.Removed -> session.send { notificationRemoved = event.message }
            }
        }

        // The desktop reconnects and syncs again. The engine calls this under
        // its lock, so the socket is closed on another thread.
        override fun abandon() {
            scope.launch { session.close() }
        }
    }

    /** Answers one request: every envelope carries the request's `seq` as `reply_to`. */
    private class RepliesTo(private val session: Session, private val requestSeq: Long) : ReplySink {
        override suspend fun send(reply: NotificationReply) {
            session.send {
                replyTo = requestSeq
                when (reply) {
                    NotificationReply.Ack -> ack = Ack.getDefaultInstance()
                    is NotificationReply.Error -> errorReply = reply.message
                    is NotificationReply.Batch -> notificationSyncBatch = reply.message
                    is NotificationReply.Image -> imageData = reply.message
                }
            }
        }
    }
}
