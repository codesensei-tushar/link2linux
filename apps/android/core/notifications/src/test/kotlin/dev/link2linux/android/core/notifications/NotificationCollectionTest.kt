package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.phoneNotification
import dev.link2linux.protocol.v1.syncCursor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationCollectionTest {
    private fun note(key: String, text: String = "") = phoneNotification {
        this.key = key
        this.text = text
    }

    private fun cursor(epoch: Long, revision: Long) = syncCursor {
        this.epoch = epoch
        this.revision = revision
    }

    private fun NotificationCollection.deltaKeys(since: dev.link2linux.protocol.v1.SyncCursor?): Triple<Boolean, List<String>, List<String>> {
        val (reset, changes) = changesSince(since)
        return Triple(
            reset,
            changes.filterIsInstance<NotificationCollection.Change.Upsert>().map { it.notification.key },
            changes.filterIsInstance<NotificationCollection.Change.Removal>().map { it.key },
        )
    }

    @Test
    fun `revisions start at one and grow by one per change`() {
        val c = NotificationCollection(7)
        assertEquals(0L, c.revision)
        assertEquals(1L, c.upsert(note("a")).revision)
        assertEquals(2L, c.upsert(note("b")).revision)
        assertEquals(3L, c.upsert(note("a", "v2")).revision)
        assertEquals(3L, c.revision)
        assertEquals(4L, c.remove("b"))
        assertNull(c.remove("b"))
        assertEquals(4L, c.revision)
    }

    @Test
    fun `an update replaces the notification in place`() {
        val c = NotificationCollection(7)
        c.upsert(note("a", "v1"))
        c.upsert(note("a", "v2"))
        assertEquals(1, c.size)
        assertEquals("v2", c["a"]!!.text)
        assertEquals(2L, c["a"]!!.revision)
    }

    @Test
    fun `no cursor, another epoch or a future revision get a snapshot`() {
        val c = NotificationCollection(7)
        c.upsert(note("a"))
        c.upsert(note("b"))
        assertTrue(c.deltaKeys(null).first)
        assertTrue(c.deltaKeys(cursor(0, 0)).first)
        assertTrue(c.deltaKeys(cursor(8, 1)).first)
        assertTrue(c.deltaKeys(cursor(7, 99)).first)
        assertEquals(listOf("a", "b"), c.deltaKeys(null).second)
    }

    @Test
    fun `a matching cursor gets a delta with changes and removals in revision order`() {
        val c = NotificationCollection(7)
        c.upsert(note("a"))
        c.upsert(note("b"))
        val at = c.cursor
        c.upsert(note("c"))
        c.remove("a")
        c.upsert(note("b", "v2"))
        val (reset, upserts, removed) = c.deltaKeys(at)
        assertFalse(reset)
        assertEquals(listOf("c", "b"), upserts)
        assertEquals(listOf("a"), removed)
    }

    @Test
    fun `a delta at the current revision is empty`() {
        val c = NotificationCollection(7)
        c.upsert(note("a"))
        val (reset, changes) = c.changesSince(c.cursor)
        assertFalse(reset)
        assertTrue(changes.isEmpty())
        val batch = c.batches(reset, changes).single()
        assertFalse(batch.more)
        assertEquals(1L, batch.cursor.revision)
    }

    @Test
    fun `a notification posted again after its removal is not also reported removed`() {
        val c = NotificationCollection(7)
        c.upsert(note("a"))
        val at = c.cursor
        c.remove("a")
        c.upsert(note("a", "back"))
        val (_, upserts, removed) = c.deltaKeys(at)
        assertEquals(listOf("a"), upserts)
        assertTrue(removed.isEmpty())
    }

    @Test
    fun `cursors older than the kept tombstones get a snapshot`() {
        val c = NotificationCollection(7, tombstoneLimit = 2)
        c.upsert(note("a"))
        c.upsert(note("b"))
        c.upsert(note("c"))
        c.upsert(note("d"))
        val old = c.cursor
        c.remove("a")
        c.remove("b")
        c.remove("c")
        // The tombstone of "a" (revision 5) was dropped; a cursor at 4 lost it.
        assertTrue(c.deltaKeys(old).first)
        // A cursor that already saw the dropped removal still gets a delta.
        assertFalse(c.deltaKeys(cursor(7, 5)).first)
    }

    @Test
    fun `a snapshot is split in batches of 100 with reset on the first and a final cursor`() {
        val c = NotificationCollection(7)
        for (i in 1..250) c.upsert(note("k$i"))
        val (reset, changes) = c.changesSince(null)
        val batches = c.batches(reset, changes)
        assertEquals(listOf(100, 100, 50), batches.map { it.notificationsCount })
        assertEquals(listOf(true, false, false), batches.map { it.reset })
        assertEquals(listOf(true, true, false), batches.map { it.more })
        assertEquals(100L, batches[0].cursor.revision)
        assertEquals(200L, batches[1].cursor.revision)
        assertEquals(250L, batches[2].cursor.revision)
        assertEquals(7L, batches[2].cursor.epoch)
    }

    @Test
    fun `batches stay under 256 KiB`() {
        val c = NotificationCollection(7)
        for (i in 1..60) c.upsert(note("k$i", "x".repeat(8 * 1024)))
        val (reset, changes) = c.changesSince(null)
        val batches = c.batches(reset, changes)
        assertTrue(batches.size > 1)
        for (b in batches) assertTrue(b.serializedSize <= Limits.SYNC_BATCH_BYTES + 64)
        assertEquals(60, batches.sumOf { it.notificationsCount })
        assertEquals(60L, batches.last().cursor.revision)
    }

    @Test
    fun `an empty snapshot is one batch that resets`() {
        val c = NotificationCollection(7)
        val (reset, changes) = c.changesSince(null)
        val batch = c.batches(reset, changes).single()
        assertTrue(batch.reset)
        assertFalse(batch.more)
        assertEquals(7L, batch.cursor.epoch)
    }

    @Test(expected = IllegalArgumentException::class)
    fun `the epoch must not be zero`() {
        NotificationCollection(0)
    }
}
