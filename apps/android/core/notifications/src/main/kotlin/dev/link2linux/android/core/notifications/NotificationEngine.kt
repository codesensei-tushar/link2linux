package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.NotificationRemovalReason
import dev.link2linux.protocol.v1.notificationPosted
import dev.link2linux.protocol.v1.notificationRemoved
import java.security.SecureRandom
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
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
 * and `NotificationRemoved` events, answers sync, dismiss, action and image
 * requests, and keeps the change log (see [NotificationCollection]).
 *
 * The platform adapter feeds it through [onListenerConnected], [onPosted],
 * [onRemoved], [onListenerDisconnected] and [onPolicyChanged]; these return at
 * once and are applied one at a time in call order. The session wires it with
 * [attach] and [handle]. All state lives behind one lock, so a live event
 * never interleaves with the batches of a sync answer.
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

        data object Disconnected : Event

        data class Posted(val snapshot: NotificationSnapshot) : Event

        data class Removed(val platformKey: String, val cause: RemovalCause) : Event

        data object PolicyChanged : Event
    }

    private class Candidate(val snapshot: NotificationSnapshot) {
        var mapped: MappedNotification? = null
        var mappedHidden = false
    }

    private class Shared(val protoKey: String, val mapped: MappedNotification)

    private val queue = Channel<Event>(Channel.UNLIMITED)
    private val lock = Mutex()

    private val _availability = MutableStateFlow(NotificationAvailability.PERMISSION_REQUIRED)

    /** For `FeatureStatus`. A change away from READY ends the live events; the desktop syncs again when READY. */
    val availability: StateFlow<NotificationAvailability> = _availability

    private val _availabilityChanges = MutableSharedFlow<NotificationAvailability>(
        extraBufferCapacity = 16,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )

    /**
     * Every change of [availability], none merged. A listener restart shows up as
     * PERMISSION_REQUIRED then READY, which is how the session learns that the
     * desktop has to sync again after the engine picked a new epoch.
     */
    val availabilityChanges: SharedFlow<NotificationAvailability> = _availabilityChanges

    private val candidates = LinkedHashMap<String, Candidate>()
    private val shared = HashMap<String, Shared>() // by protocol key
    private val sharedByPlatformKey = HashMap<String, String>()
    private var collection = NotificationCollection(newEpoch())
    private var transport: NotificationTransport? = null
    private var subscribed = false

    init {
        _availability.value = currentAvailability()
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

    /** The listener disconnected or access was revoked. */
    fun onListenerDisconnected() {
        queue.trySend(Event.Disconnected)
    }

    fun onPosted(snapshot: NotificationSnapshot) {
        queue.trySend(Event.Posted(snapshot))
    }

    fun onRemoved(platformKey: String, cause: RemovalCause) {
        queue.trySend(Event.Removed(platformKey, cause))
    }

    /** The user changed the policy: feature on or off, an app filter, or access. */
    fun onPolicyChanged() {
        queue.trySend(Event.PolicyChanged)
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
        refuse()?.let { return@withLock replies.send(it) }
        when (request) {
            is NotificationRequest.Sync -> handleSync(request, replies)
            else -> replies.send(failure(ErrorCode.ERROR_CODE_UNSUPPORTED))
        }
    }

    // Events

    private suspend fun apply(event: Event) {
        when (event) {
            is Event.Connected -> {
                resetCollection()
                candidates.clear()
                for (s in event.active) if (s.packageName != ownPackage) candidates[s.key] = Candidate(s)
                refreshAvailability(transition = false)
                reconcileAll(quiet = true)
            }
            Event.Disconnected -> {
                candidates.clear()
                resetCollection()
                refreshAvailability(transition = false)
            }
            Event.PolicyChanged -> refreshAvailability(transition = true)
            is Event.Posted -> {
                val s = event.snapshot
                if (s.packageName == ownPackage) return
                candidates[s.key] = Candidate(s)
                reconcileWithGroup(s.key, s.packageName, s.groupKey, quiet = false)
            }
            is Event.Removed -> {
                val old = candidates.remove(event.platformKey)
                val reason = reasonOf(event.cause)
                removeShared(event.platformKey, reason)
                if (old != null) reconcileWithGroup(null, old.snapshot.packageName, old.snapshot.groupKey, quiet = false)
            }
        }
    }

    private fun currentAvailability() = when {
        !policy.enabled -> NotificationAvailability.DISABLED
        !policy.accessGranted -> NotificationAvailability.PERMISSION_REQUIRED
        else -> NotificationAvailability.READY
    }

    private suspend fun refreshAvailability(transition: Boolean) {
        val before = _availability.value
        val now = currentAvailability()
        if (now != before) {
            _availability.value = now
            _availabilityChanges.tryEmit(now)
        }
        if (!transition) return
        if (now != NotificationAvailability.READY) {
            if (before == NotificationAvailability.READY) resetCollection()
        } else {
            reconcileAll(quiet = true)
        }
    }

    /** A new epoch and nothing shared: the desktop gets a snapshot on its next sync. */
    private fun resetCollection() {
        collection = NotificationCollection(newEpoch())
        shared.clear()
        sharedByPlatformKey.clear()
        subscribed = false
        images.retain(emptySet())
    }

    private suspend fun reconcileAll(quiet: Boolean) {
        for (key in candidates.keys.toList()) reconcile(key, quiet)
        // Anything shared that no candidate backs any more.
        for (key in sharedByPlatformKey.keys.toList()) if (key !in candidates) {
            removeShared(key, NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_NOT_SHARED)
        }
    }

    private suspend fun reconcileWithGroup(platformKey: String?, pkg: String, groupKey: String?, quiet: Boolean) {
        if (platformKey != null) reconcile(platformKey, quiet)
        if (groupKey == null) return
        for ((key, c) in candidates.entries.toList()) {
            val s = c.snapshot
            // A summary that appears or goes away never alerts.
            if (s.isGroupSummary && s.packageName == pkg && s.groupKey == groupKey) reconcile(key, quiet = true)
        }
    }

    private fun shouldShare(s: NotificationSnapshot): Boolean {
        if (_availability.value != NotificationAvailability.READY) return false
        if (s.isFromOtherProfile || s.isLocalOnly || policy.isExcluded(s.packageName)) return false
        // A group summary stays out while any child of its group is shared.
        if (s.isGroupSummary && s.groupKey != null) {
            val hasChild = candidates.values.any {
                val o = it.snapshot
                !o.isGroupSummary && o.packageName == s.packageName && o.groupKey == s.groupKey && shouldShare(o)
            }
            if (hasChild) return false
        }
        return true
    }

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
        val previous = shared[protoKey]
        val previousNotification = previous?.let { collection[protoKey] }
        shared[protoKey] = Shared(protoKey, mapped)
        sharedByPlatformKey[platformKey] = protoKey
        if (previousNotification != null &&
            previousNotification.toBuilder().clearRevision().build() == mapped.notification
        ) {
            return
        }
        val stored = collection.upsert(mapped.notification)
        images.retain(shared.values.flatMapTo(HashSet()) { it.mapped.imageHashes })
        val alert = !quiet && if (previousNotification == null) mapped.alertWhenNew else mapped.alertOnUpdate
        emit(NotificationEvent.Posted(notificationPosted {
            notification = stored
            this.alert = alert
        }))
    }

    private suspend fun removeShared(platformKey: String, reason: NotificationRemovalReason) {
        val protoKey = sharedByPlatformKey.remove(platformKey) ?: return
        shared.remove(protoKey)
        val revision = collection.remove(protoKey) ?: return
        images.retain(shared.values.flatMapTo(HashSet()) { it.mapped.imageHashes })
        emit(NotificationEvent.Removed(notificationRemoved {
            key = protoKey
            this.revision = revision
            this.reason = reason
        }))
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

    private fun refuse(): NotificationReply.Error? = when (_availability.value) {
        NotificationAvailability.READY -> null
        NotificationAvailability.DISABLED -> failure(ErrorCode.ERROR_CODE_DISABLED, "notifications are turned off")
        NotificationAvailability.PERMISSION_REQUIRED ->
            failure(ErrorCode.ERROR_CODE_PERMISSION_REQUIRED, "notification access is not granted", NOTIFICATION_ACCESS)
    }

    private suspend fun handleSync(request: NotificationRequest.Sync, replies: ReplySink) {
        val since = if (request.message.hasSince()) request.message.since else null
        val (reset, changes) = collection.changesSince(since)
        for (batch in collection.batches(reset, changes)) replies.send(NotificationReply.Batch(batch))
        subscribed = true
    }

    companion object {
        /** `ErrorReply.permission` for the missing notification access. */
        const val NOTIFICATION_ACCESS = "notification-access"

        private val random = SecureRandom()

        fun randomEpoch(): Long {
            while (true) {
                val v = random.nextLong()
                if (v != 0L) return v
            }
        }
    }
}
