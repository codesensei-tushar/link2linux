package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ErrorCode
import java.security.SecureRandom
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Holds the notifications the phone shares and talks the notifications part
 * of the protocol: it keeps the change log (see [NotificationCollection]) and
 * answers the desktop's requests.
 *
 * The platform adapter feeds it through [onListenerConnected], [onPosted] and
 * [onRemoved]; these return at once and are applied one at a time in call
 * order. The session calls [handle]. All state lives behind one lock.
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

        data class Removed(val platformKey: String) : Event
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

    fun onRemoved(platformKey: String) {
        queue.trySend(Event.Removed(platformKey))
    }

    // Session side

    /** Carries out one request of the desktop and sends its answers to [replies]. */
    suspend fun handle(request: NotificationRequest, replies: ReplySink) = lock.withLock {
        when (request) {
            is NotificationRequest.Sync -> handleSync(request, replies)
            else -> replies.send(failure(ErrorCode.ERROR_CODE_UNSUPPORTED))
        }
    }

    // Events

    private fun apply(event: Event) {
        when (event) {
            is Event.Connected -> {
                collection = NotificationCollection(newEpoch())
                shared.clear()
                sharedByPlatformKey.clear()
                candidates.clear()
                for (s in event.active) if (s.packageName != ownPackage) candidates[s.key] = Candidate(s)
                for (key in candidates.keys.toList()) reconcile(key)
            }
            is Event.Posted -> {
                val s = event.snapshot
                if (s.packageName == ownPackage) return
                candidates[s.key] = Candidate(s)
                reconcile(s.key)
            }
            is Event.Removed -> {
                candidates.remove(event.platformKey)
                removeShared(event.platformKey)
            }
        }
    }

    private fun shouldShare(s: NotificationSnapshot): Boolean =
        !(s.isFromOtherProfile || s.isLocalOnly || policy.isExcluded(s.packageName))

    private fun reconcile(platformKey: String) {
        val candidate = candidates[platformKey]
        if (candidate == null || !shouldShare(candidate.snapshot)) {
            removeShared(platformKey)
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
        collection.upsert(mapped.notification)
        images.retain(shared.values.flatMapTo(HashSet()) { it.mapped.imageHashes })
    }

    private fun removeShared(platformKey: String) {
        val protoKey = sharedByPlatformKey.remove(platformKey) ?: return
        shared.remove(protoKey)
        collection.remove(protoKey) ?: return
        images.retain(shared.values.flatMapTo(HashSet()) { it.mapped.imageHashes })
    }

    // Requests

    private suspend fun handleSync(request: NotificationRequest.Sync, replies: ReplySink) {
        val since = if (request.message.hasSince()) request.message.since else null
        val (reset, changes) = collection.changesSince(since)
        for (batch in collection.batches(reset, changes)) replies.send(NotificationReply.Batch(batch))
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
