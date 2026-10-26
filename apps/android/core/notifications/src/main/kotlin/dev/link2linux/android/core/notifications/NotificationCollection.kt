package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.NotificationSyncBatch
import dev.link2linux.protocol.v1.PhoneNotification
import dev.link2linux.protocol.v1.SyncCursor
import dev.link2linux.protocol.v1.notificationSyncBatch
import dev.link2linux.protocol.v1.syncCursor

/**
 * The notifications the phone shares, with the change log of docs/protocol.md
 * "Delta sync": an epoch, a revision that grows by one per change, and
 * tombstones for removals. Not thread-safe; the engine serializes access.
 */
class NotificationCollection(
    val epoch: Long,
    private val tombstoneLimit: Int = TOMBSTONES,
) {
    init {
        require(epoch != 0L) { "The epoch must not be 0" }
    }

    /** One entry of a delta: a notification that changed, or a key that went away. */
    sealed interface Change {
        val revision: Long

        data class Upsert(val notification: PhoneNotification) : Change {
            override val revision: Long get() = notification.revision
        }

        data class Removal(val key: String, override val revision: Long) : Change
    }

    private class Tombstone(val key: String, val revision: Long)

    private val items = LinkedHashMap<String, PhoneNotification>()
    private val tombstones = ArrayDeque<Tombstone>()

    // Highest revision of a tombstone already dropped: cursors below it cannot get a delta.
    private var droppedUpTo = 0L

    var revision: Long = 0
        private set

    val size: Int get() = items.size

    val cursor: SyncCursor
        get() = syncCursor {
            epoch = this@NotificationCollection.epoch
            revision = this@NotificationCollection.revision
        }

    operator fun get(key: String): PhoneNotification? = items[key]

    val keys: Set<String> get() = items.keys

    /** Stores a new or changed notification at the next revision and returns it with that revision set. */
    fun upsert(notification: PhoneNotification): PhoneNotification {
        revision += 1
        val stored = notification.toBuilder().setRevision(revision).build()
        items.remove(stored.key)
        items[stored.key] = stored
        tombstones.removeAll { it.key == stored.key }
        return stored
    }

    /** Removes a notification and returns the revision of the removal, or null if it was not there. */
    fun remove(key: String): Long? {
        if (items.remove(key) == null) return null
        revision += 1
        tombstones.addLast(Tombstone(key, revision))
        while (tombstones.size > tombstoneLimit) droppedUpTo = tombstones.removeFirst().revision
        return revision
    }

    /** True when [since] can be answered with a delta. */
    fun canDelta(since: SyncCursor?): Boolean =
        since != null && since.epoch == epoch && since.revision <= revision && since.revision >= droppedUpTo

    /**
     * What the follower at [since] has to apply: the changes after its revision
     * in revision order, or, when it cannot be answered with a delta, a
     * snapshot of every notification. [reset] tells which one it is.
     */
    fun changesSince(since: SyncCursor?): Pair<Boolean, List<Change>> {
        if (!canDelta(since)) {
            return true to items.values.map<PhoneNotification, Change> { Change.Upsert(it) }.sortedBy { it.revision }
        }
        val from = since!!.revision
        val changes = ArrayList<Change>()
        items.values.filter { it.revision > from }.mapTo(changes) { Change.Upsert(it) }
        tombstones.filter { it.revision > from }.mapTo(changes) { Change.Removal(it.key, it.revision) }
        changes.sortBy { it.revision }
        return false to changes
    }

    /**
     * Splits [changes] into batches of at most 100 notifications or 256 KiB.
     * Each batch carries the cursor reached by applying it, so the last one
     * ends at the collection's current revision.
     */
    fun batches(reset: Boolean, changes: List<Change>): List<NotificationSyncBatch> {
        val result = ArrayList<NotificationSyncBatch>()
        var upserts = ArrayList<PhoneNotification>()
        var removed = ArrayList<String>()
        var bytes = 0
        var lastRevision = 0L

        fun flush(more: Boolean) {
            val first = result.isEmpty()
            val cursorRevision = if (more) lastRevision else revision
            result += notificationSyncBatch {
                this.reset = reset && first
                notifications += upserts
                removedKeys += removed
                cursor = syncCursor {
                    epoch = this@NotificationCollection.epoch
                    revision = cursorRevision
                }
                this.more = more
            }
            upserts = ArrayList()
            removed = ArrayList()
            bytes = 0
        }

        for (change in changes) {
            val size = when (change) {
                is Change.Upsert -> change.notification.serializedSize
                is Change.Removal -> change.key.length + 8
            }
            val full = upserts.size >= Limits.SYNC_BATCH_NOTIFICATIONS ||
                (bytes > 0 && bytes + size > Limits.SYNC_BATCH_BYTES)
            if (full) flush(more = true)
            when (change) {
                is Change.Upsert -> upserts += change.notification
                is Change.Removal -> removed += change.key
            }
            bytes += size
            lastRevision = change.revision
        }
        flush(more = false)
        return result
    }

    companion object {
        /** The protocol asks for the last 500 removals at least. */
        const val TOMBSTONES = 500
    }
}
