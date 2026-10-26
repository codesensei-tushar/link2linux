package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.NotificationRemovalReason
import dev.link2linux.protocol.v1.notificationPosted
import dev.link2linux.protocol.v1.notificationRemoved
import java.security.SecureRandom
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Why the platform removed a notification, reduced to what the protocol can tell apart. */
enum class RemovalCause {
    USER_DISMISSED,
    OPENED,
    CANCELLED_BY_APP,
    TIMED_OUT,
    UNKNOWN,
    ;

    companion object {
        /** From `NotificationListenerService.REASON_*`. */
        fun fromAndroidReason(reason: Int): RemovalCause = when (reason) {
            1 -> OPENED // REASON_CLICK
            2, 3 -> USER_DISMISSED // REASON_CANCEL, REASON_CANCEL_ALL
            8, 9, 12, 13 -> CANCELLED_BY_APP // APP_CANCEL, APP_CANCEL_ALL, GROUP_SUMMARY_CANCELED, GROUP_OPTIMIZATION
            19 -> TIMED_OUT // REASON_TIMEOUT
            else -> UNKNOWN
        }
    }
}

/**
 * Holds the notifications the phone shares and talks the notifications part
 * of the protocol: it turns what the listener reports into `NotificationPosted`
 * and `NotificationRemoved` events and keeps the change log (see
 * [NotificationCollection]). A live event goes out only once the desktop has
 * synced, and never in the middle of a sync answer.
 *
 * The platform adapter feeds it through [onListenerConnected], [onPosted] and
 * [onRemoved]; these return at once and are applied one at a time in call
 * order. The session wires it with [attach] and [handle]. All state lives
 * behind one lock.
 *
 * @param ownPackage notifications of this package are never shared.
 */
class NotificationEngine(
    scope: CoroutineScope,
    private val policy: NotificationPolicy,
    private val environment: NotificationEnvironment,
    private val mapper: NotificationMapper,
    private val images: ImageStore,
    private val canceller: NotificationCanceller,
    private val ownPackage: String,
    private val clock: () -> Long = System::currentTimeMillis,
    private val newEpoch: () -> Long = ::randomEpoch,
    private val onError: (Throwable) -> Unit = {},
) {
    private sealed interface Event {
        data class Connected(val active: List<NotificationSnapshot>) : Event

        data class Posted(val snapshot: NotificationSnapshot) : Event

        data class Removed(val platformKey: String, val cause: RemovalCause) : Event
    }

    private class Candidate(val snapshot: NotificationSnapshot) {
        var mapped: MappedNotification? = null
        var mappedHidden = false
    }

    private class Shared(val protoKey: String, val mapped: MappedNotification)

    private val queue = Channel<Event>(Channel.UNLIMITED)
    private val lock = Mutex()

    private val candidates = LinkedHashMap<String, Candidate>()
    private val shared = HashMap<String, Shared>() // by protocol key
    private val sharedByPlatformKey = HashMap<String, String>()
    private var collection = NotificationCollection(newEpoch())
    private var transport: NotificationTransport? = null
    private var subscribed = false

    init {
        scope.launch {
            for (event in queue) {
                try {
                    lock.withLock { apply(event) }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    onError(e)
                }
            }
        }
    }

    // Platform side

    /** The listener connected, with the notifications active at that moment. Starts a new epoch. */
    fun onListenerConnected(active: List<NotificationSnapshot>) {
        queue.trySend(Event.Connected(active))
    }

    fun onPosted(snapshot: NotificationSnapshot) {
        queue.trySend(Event.Posted(snapshot))
    }

    fun onRemoved(platformKey: String, cause: RemovalCause) {
        queue.trySend(Event.Removed(platformKey, cause))
    }

    // Session side

    /** A session is up: live events go to [transport], once the desktop has synced. */
    suspend fun attach(transport: NotificationTransport) = lock.withLock {
        this.transport = transport
        subscribed = false
    }

    /** The session ended. */
    suspend fun detach() = lock.withLock {
        transport = null
        subscribed = false
    }

    /** Carries out one request of the desktop and sends its answers to [replies]. */
    suspend fun handle(request: NotificationRequest, replies: ReplySink) = lock.withLock {
        when (request) {
            is NotificationRequest.Sync -> handleSync(request, replies)
            else -> replies.send(failure(ErrorCode.ERROR_CODE_UNSUPPORTED))
        }
    }

    // Events

    private suspend fun apply(event: Event) {
        when (event) {
            is Event.Connected -> {
                collection = NotificationCollection(newEpoch())
                subscribed = false
                shared.clear()
                sharedByPlatformKey.clear()
                candidates.clear()
                for (s in event.active) if (s.packageName != ownPackage) candidates[s.key] = Candidate(s)
                for (key in candidates.keys.toList()) reconcile(key, quiet = true)
            }
            is Event.Posted -> {
                val s = event.snapshot
                if (s.packageName == ownPackage) return
                candidates[s.key] = Candidate(s)
                reconcile(s.key, quiet = false)
            }
            is Event.Removed -> {
                candidates.remove(event.platformKey)
                removeShared(event.platformKey, reasonOf(event.cause))
            }
        }
    }

    private fun shouldShare(s: NotificationSnapshot): Boolean =
        !(s.isFromOtherProfile || s.isLocalOnly || policy.isExcluded(s.packageName))

    private suspend fun reconcile(platformKey: String, quiet: Boolean) {
        val candidate = candidates[platformKey]
        if (candidate == null || !shouldShare(candidate.snapshot)) {
            removeShared(platformKey, NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_NOT_SHARED)
            return
        }
        val hidden = policy.isContentHidden(candidate.snapshot.packageName)
        var mapped = candidate.mapped
        if (mapped == null || candidate.mappedHidden != hidden) {
            mapped = mapper.map(candidate.snapshot, hidden)
            candidate.mapped = mapped
            candidate.mappedHidden = hidden
        }
        val protoKey = mapped.notification.key
        val previous = collection[protoKey]
        shared[protoKey] = Shared(protoKey, mapped)
        sharedByPlatformKey[platformKey] = protoKey
        if (previous != null && previous.toBuilder().clearRevision().build() == mapped.notification) return
        val stored = collection.upsert(mapped.notification)
        images.retain(shared.values.flatMapTo(HashSet()) { it.mapped.imageHashes })
        val alert = !quiet && if (previous == null) mapped.alertWhenNew else mapped.alertOnUpdate
        emit(
            NotificationEvent.Posted(
                notificationPosted {
                    notification = stored
                    this.alert = alert
                },
            ),
        )
    }

    private suspend fun removeShared(platformKey: String, reason: NotificationRemovalReason) {
        val protoKey = sharedByPlatformKey.remove(platformKey) ?: return
        shared.remove(protoKey)
        val revision = collection.remove(protoKey) ?: return
        images.retain(shared.values.flatMapTo(HashSet()) { it.mapped.imageHashes })
        emit(
            NotificationEvent.Removed(
                notificationRemoved {
                    key = protoKey
                    this.revision = revision
                    this.reason = reason
                },
            ),
        )
    }

    private fun reasonOf(cause: RemovalCause) = when (cause) {
        RemovalCause.USER_DISMISSED -> NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_DISMISSED
        RemovalCause.OPENED -> NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_OPENED
        RemovalCause.CANCELLED_BY_APP -> NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_CANCELLED_BY_APP
        RemovalCause.TIMED_OUT -> NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_TIMED_OUT
        RemovalCause.UNKNOWN -> NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_UNSPECIFIED
    }

    private suspend fun emit(event: NotificationEvent) {
        val t = transport
        if (t == null || !subscribed) return
        try {
            t.send(event)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // The connection is going away. Stop sending, so the desktop never sees events with gaps
            // before it syncs again.
            subscribed = false
            onError(e)
        }
    }

    // Requests

    private suspend fun handleSync(request: NotificationRequest.Sync, replies: ReplySink) {
        val since = if (request.message.hasSince()) request.message.since else null
        val (reset, changes) = collection.changesSince(since)
        for (batch in collection.batches(reset, changes)) replies.send(NotificationReply.Batch(batch))
        subscribed = true
    }

    companion object {
        private val random = SecureRandom()

        fun randomEpoch(): Long {
            while (true) {
                val v = random.nextLong()
                if (v != 0L) return v
            }
        }
    }
}
