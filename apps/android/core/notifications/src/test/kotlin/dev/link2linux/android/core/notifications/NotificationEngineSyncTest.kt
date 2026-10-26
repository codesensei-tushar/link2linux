package dev.link2linux.android.core.notifications

import dev.link2linux.android.core.notifications.EngineHarness.Companion.snap
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.NotificationRemovalReason
import dev.link2linux.protocol.v1.imageRequest
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationEngineSyncTest {
    @Test
    fun `a sync after connecting answers with a snapshot of what is shared`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"), snap("b"))
        val answer = h.sync()
        with(h) {
            assertEquals(listOf("a", "b"), answer.keys())
            assertTrue((answer.replies.first() as NotificationReply.Batch).message.reset)
            assertEquals(2L, answer.cursor().revision)
            assertNotEquals(0L, answer.cursor().epoch)
        }
    }

    @Test
    fun `own notifications, work profile, local only and excluded apps are not shared`() = runTest {
        val h = EngineHarness(this)
        h.policy.excluded += "org.muted"
        h.connect(
            snap("own", pkg = "dev.link2linux.android"),
            snap("work").copy(isFromOtherProfile = true),
            snap("local").copy(isLocalOnly = true),
            snap("muted", pkg = "org.muted"),
            snap("ok"),
        )
        with(h) { assertEquals(listOf("ok"), sync().keys()) }
    }

    @Test
    fun `posts and removals change what a later sync returns`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"))
        h.post(snap("b"))
        h.post(snap("a", text = "v2"))
        h.remove("b")
        h.remove("unknown")
        val answer = h.sync()
        with(h) {
            assertEquals(listOf("a"), answer.keys())
            assertEquals(4L, answer.cursor().revision)
        }
    }

    @Test
    fun `live events are sent only after the desktop synced`() = runTest {
        val h = EngineHarness(this)
        h.connect()
        h.attach()
        h.post(snap("early"))
        assertTrue(h.transport.events.isEmpty())
        h.sync()
        h.post(snap("late"))
        assertEquals(listOf("late"), h.transport.posted.map { it.notification.key })
        assertEquals(2L, h.transport.posted.single().notification.revision)
    }

    @Test
    fun `new notifications alert only at default importance or higher, updates follow the app's choice`() = runTest {
        val h = EngineHarness(this)
        h.connect()
        h.attach()
        h.sync()
        h.post(snap("loud"))
        h.post(snap("quiet", importance = AndroidValues.IMPORTANCE_LOW))
        h.post(snap("loud", text = "v2"))
        h.post(snap("once", importance = AndroidValues.IMPORTANCE_HIGH).copy(onlyAlertOnce = true))
        h.post(snap("once", text = "v2").copy(onlyAlertOnce = true))
        assertEquals(listOf(true, false, true, true, false), h.transport.posted.map { it.alert })
    }

    @Test
    fun `an identical repost sends nothing`() = runTest {
        val h = EngineHarness(this)
        h.connect()
        h.attach()
        h.sync()
        h.post(snap("a"))
        h.post(snap("a"))
        assertEquals(1, h.transport.posted.size)
        assertEquals(1L, with(h) { sync().cursor().revision })
    }

    @Test
    fun `removals carry the reason`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"), snap("b"), snap("c"), snap("d"))
        h.attach()
        h.sync()
        h.remove("a", RemovalCause.USER_DISMISSED)
        h.remove("b", RemovalCause.OPENED)
        h.remove("c", RemovalCause.CANCELLED_BY_APP)
        h.remove("d", RemovalCause.TIMED_OUT)
        h.remove("unknown")
        assertEquals(
            listOf(
                NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_DISMISSED,
                NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_OPENED,
                NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_CANCELLED_BY_APP,
                NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_TIMED_OUT,
            ),
            h.transport.removed.map { it.reason },
        )
        assertEquals(listOf(5L, 6L, 7L, 8L), h.transport.removed.map { it.revision })
    }

    @Test
    fun `a reconnecting desktop with its cursor gets a delta`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"), snap("b"))
        val first = h.sync()
        h.post(snap("c"))
        h.remove("a")
        val delta = h.sync(with(h) { first.cursor() })
        with(h) {
            assertFalse((delta.replies.first() as NotificationReply.Batch).message.reset)
            assertEquals(listOf("c"), delta.keys())
            assertEquals(listOf("a"), delta.replies.filterIsInstance<NotificationReply.Batch>().flatMap { it.message.removedKeysList })
            assertEquals(4L, delta.cursor().revision)
        }
    }

    @Test
    fun `restarting the listener starts a new epoch and the old cursor gets a snapshot`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"))
        val old = with(h) { sync().cursor() }
        h.connect(snap("a"), snap("b"))
        val answer = h.sync(old)
        with(h) {
            assertTrue((answer.replies.first() as NotificationReply.Batch).message.reset)
            assertEquals(listOf("a", "b"), answer.keys())
            assertNotEquals(old.epoch, answer.cursor().epoch)
        }
    }

    @Test
    fun `a failing transport stops the events until the next sync`() = runTest {
        val h = EngineHarness(this)
        h.connect()
        h.attach()
        h.sync()
        h.transport.failWith = java.io.IOException("gone")
        h.post(snap("a"))
        h.transport.failWith = null
        h.post(snap("b"))
        assertTrue(h.transport.events.isEmpty())
        assertEquals(1, h.errors.size)
        h.sync()
        h.post(snap("c"))
        assertEquals(listOf("c"), h.transport.posted.map { it.notification.key })
    }

    @Test
    fun `a request the engine does not handle yet is unsupported`() = runTest {
        val sink = RecordingSink()
        EngineHarness(this).engine.handle(NotificationRequest.Image(imageRequest { }), sink)
        sink.assertError(ErrorCode.ERROR_CODE_UNSUPPORTED)
    }
}
