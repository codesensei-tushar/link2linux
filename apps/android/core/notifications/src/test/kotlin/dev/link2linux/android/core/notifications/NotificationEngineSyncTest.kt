package dev.link2linux.android.core.notifications

import dev.link2linux.android.core.notifications.EngineHarness.Companion.snap
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.NotificationContent
import dev.link2linux.protocol.v1.NotificationRemovalReason
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
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
    fun `a group summary stays out while a child is shared and returns when the children are gone`() = runTest {
        val h = EngineHarness(this)
        val summary = snap("sum").copy(groupKey = "g", isGroupSummary = true)
        val child = snap("child").copy(groupKey = "g")
        h.connect(summary)
        h.attach()
        with(h) { assertEquals(listOf("sum"), sync().keys()) }
        h.post(child)
        assertEquals(listOf("child"), h.transport.posted.map { it.notification.key })
        assertEquals(
            listOf("sum" to NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_NOT_SHARED),
            h.transport.removed.map { it.key to it.reason },
        )
        h.remove("child")
        assertEquals(listOf("child", "sum"), h.transport.posted.map { it.notification.key })
        assertFalse(h.transport.posted.last().alert)
    }

    @Test
    fun `excluding an app reports its notifications as not shared`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a", pkg = "org.chat"), snap("b", pkg = "org.mail"))
        h.attach()
        h.sync()
        h.policy.excluded += "org.chat"
        h.changePolicy()
        assertEquals(
            listOf("a" to NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_NOT_SHARED),
            h.transport.removed.map { it.key to it.reason },
        )
        h.policy.excluded.clear()
        h.changePolicy()
        assertEquals(listOf("a"), h.transport.posted.map { it.notification.key })
        assertFalse(h.transport.posted.single().alert)
    }

    @Test
    fun `hiding an app's content re-sends it as hidden by the user`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a", pkg = "org.chat", text = "secret"))
        h.attach()
        h.sync()
        h.policy.hidden += "org.chat"
        h.changePolicy()
        val n = h.transport.posted.single().notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_HIDDEN_BY_USER, n.content)
        assertEquals("", n.text)
    }

    @Test
    fun `an update that brings a one-time code is re-sent without its content`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a", text = "Signing you in"))
        h.attach()
        h.sync()
        h.post(snap("a", text = "Your login code is 482913"))
        val n = h.transport.posted.single().notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_ONE_TIME_CODE, n.content)
        assertEquals("", n.title)
        assertEquals("", n.text)
    }

    @Test
    fun `turning code hiding off re-sends codes in full`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a", text = "Your code is 482913"))
        h.attach()
        h.sync()
        h.policy.hideOneTimeCodes = false
        h.changePolicy()
        val n = h.transport.posted.single().notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_FULL, n.content)
        assertEquals("Your code is 482913", n.text)
    }

    @Test
    fun `a request while the feature is off or access is missing is refused`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"))
        h.policy.enabled = false
        h.changePolicy()
        h.sync().assertError(ErrorCode.ERROR_CODE_DISABLED)
        h.policy.enabled = true
        h.policy.accessGranted = false
        h.changePolicy()
        val denied = h.sync()
        denied.assertError(ErrorCode.ERROR_CODE_PERMISSION_REQUIRED)
        assertEquals("notification-access", (denied.single as NotificationReply.Error).message.permission)
        assertEquals(NotificationAvailability.PERMISSION_REQUIRED, h.engine.availability.value)
    }

    @Test
    fun `turning the feature off ends live events and turning it on gives a new epoch`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"))
        h.attach()
        val before = with(h) { sync().cursor() }
        h.policy.enabled = false
        h.changePolicy()
        h.post(snap("b"))
        assertTrue(h.transport.events.isEmpty())
        h.policy.enabled = true
        h.changePolicy()
        val answer = h.sync(before)
        with(h) {
            assertEquals(listOf("a", "b"), answer.keys())
            assertNotEquals(before.epoch, answer.cursor().epoch)
        }
    }

    @Test
    fun `losing the listener empties the collection`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"))
        h.policy.accessGranted = false
        h.engine.onListenerDisconnected()
        h.changePolicy()
        h.policy.accessGranted = true
        h.connect()
        with(h) { assertTrue(sync().keys().isEmpty()) }
    }

    @Test
    fun `a listener restart shows up as two availability changes`() = runTest {
        val h = EngineHarness(this)
        h.connect(snap("a"))
        val seen = mutableListOf<NotificationAvailability>()
        val job = launch(UnconfinedTestDispatcher(testScheduler)) { h.engine.availabilityChanges.toList(seen) }
        h.policy.accessGranted = false
        h.engine.onListenerDisconnected()
        h.policy.accessGranted = true
        h.connect(snap("a"))
        job.cancel()
        assertEquals(listOf(NotificationAvailability.PERMISSION_REQUIRED, NotificationAvailability.READY), seen)
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
    fun `each attached session gets live events once its own desktop synced`() = runTest {
        val h = EngineHarness(this)
        val other = FakeTransport()
        h.connect()
        h.attach()
        h.attach(other)
        h.sync()
        h.post(snap("a"))
        // The other desktop has no snapshot yet, so events would leave it with gaps.
        assertEquals(listOf("a"), h.transport.posted.map { it.notification.key })
        assertTrue(other.events.isEmpty())
        h.sync(from = other)
        h.post(snap("b"))
        assertEquals(listOf("a", "b"), h.transport.posted.map { it.notification.key })
        assertEquals(listOf("b"), other.posted.map { it.notification.key })
    }

    @Test
    fun `a session that fails or ends leaves the others their live events`() = runTest {
        val h = EngineHarness(this)
        val other = FakeTransport()
        h.connect()
        h.attach()
        h.attach(other)
        h.sync()
        h.sync(from = other)
        other.failWith = java.io.IOException("gone")
        h.post(snap("a"))
        other.failWith = null
        assertEquals(listOf("a"), h.transport.posted.map { it.notification.key })
        assertEquals(1, h.errors.size)

        h.engine.detach(h.transport)
        h.sync(from = other)
        h.post(snap("b"))
        assertEquals(listOf("a"), h.transport.posted.map { it.notification.key })
        assertEquals(listOf("b"), other.posted.map { it.notification.key })

        // A sync from a session that is not attached is answered, but subscribes nothing.
        val ended = FakeTransport()
        with(h) { assertEquals(listOf("a", "b"), sync(from = ended).keys()) }
        h.post(snap("c"))
        assertTrue(ended.events.isEmpty())
    }
}
