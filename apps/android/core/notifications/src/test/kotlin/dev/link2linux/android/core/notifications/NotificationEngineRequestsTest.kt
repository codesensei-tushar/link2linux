package dev.link2linux.android.core.notifications

import com.google.protobuf.ByteString
import dev.link2linux.android.core.notifications.EngineHarness.Companion.idempotency
import dev.link2linux.android.core.notifications.EngineHarness.Companion.snap
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ImageFormat
import dev.link2linux.protocol.v1.NotificationRemovalReason
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationEngineRequestsTest {
    private class Recorder(val outcome: ActionOutcome = ActionOutcome.SENT) : ActionInvoker {
        val calls = mutableListOf<Pair<String?, Boolean>>()

        override fun invoke(replyText: String?, allowBackgroundStart: Boolean): ActionOutcome {
            calls += replyText to allowBackgroundStart
            return outcome
        }
    }

    private suspend fun ready(scope: TestScope, vararg active: NotificationSnapshot): EngineHarness {
        val h = EngineHarness(scope)
        h.connect(*active)
        h.attach()
        h.sync()
        return h
    }

    private fun withAction(a: ActionSnapshot, key: String = "n") = snap(key).copy(actions = listOf(a))

    private suspend fun actionId(h: EngineHarness, key: String = "n", index: Int = 0): String {
        val sink = h.sync()
        return sink.replies.filterIsInstance<NotificationReply.Batch>().flatMap { it.message.notificationsList }
            .first { it.key == key }.actionsList[index].id
    }

    // Dismiss

    @Test
    fun `dismiss cancels the notification and the removal is reported as from the desktop`() = runTest {
        val h = ready(this, snap("n"))
        h.dismiss("n").also { assertEquals(NotificationReply.Ack, it.single) }
        assertEquals(listOf("n"), h.canceller.cancelled)
        // Android then reports the removal.
        h.remove("n", RemovalCause.UNKNOWN)
        assertEquals(
            NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_DISMISSED_FROM_DESKTOP,
            h.transport.removed.single().reason,
        )
    }

    @Test
    fun `a removal the desktop did not ask for keeps its own reason`() = runTest {
        val h = ready(this, snap("n"), snap("m"))
        h.dismiss("n")
        h.remove("m", RemovalCause.USER_DISMISSED)
        assertEquals(NotificationRemovalReason.NOTIFICATION_REMOVAL_REASON_DISMISSED, h.transport.removed.single().reason)
    }

    @Test
    fun `dismiss of an unknown notification is not found`() = runTest {
        ready(this).dismiss("nope").assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
    }

    @Test
    fun `dismiss of a notification that cannot be dismissed is an invalid argument`() = runTest {
        val h = ready(this, snap("n").copy(isOngoing = true))
        h.dismiss("n").assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
        assertTrue(h.canceller.cancelled.isEmpty())
    }

    @Test
    fun `dismiss is declined when dismiss sync is off on the phone`() = runTest {
        val h = ready(this, snap("n"))
        h.policy.dismissSync = false
        h.dismiss("n").assertError(ErrorCode.ERROR_CODE_DECLINED)
        assertTrue(h.canceller.cancelled.isEmpty())
    }

    @Test
    fun `a failed cancel is unavailable and can be retried`() = runTest {
        val h = ready(this, snap("n"))
        h.canceller.result = false
        h.dismiss("n").assertError(ErrorCode.ERROR_CODE_UNAVAILABLE)
        h.canceller.result = true
        assertEquals(NotificationReply.Ack, h.dismiss("n").single)
        assertEquals(2, h.canceller.cancelled.size)
    }

    @Test
    fun `a retried dismiss is answered again without cancelling twice`() = runTest {
        val h = ready(this, snap("n"))
        h.dismiss("n")
        h.remove("n")
        // The notification is gone now, but the same key gets the same answer.
        assertEquals(NotificationReply.Ack, h.dismiss("n").single)
        assertEquals(1, h.canceller.cancelled.size)
        // A new key is a new request.
        h.dismiss("n", idem = idempotency(9)).assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
    }

    @Test
    fun `remembered answers expire after 24 hours`() = runTest {
        val h = ready(this, snap("n"))
        h.dismiss("n")
        h.remove("n")
        h.now += 25L * 60 * 60 * 1000
        h.dismiss("n", idem = idempotency(1)) { createdUnixMs = h.now }.assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
    }

    @Test
    fun `a bad idempotency key or an old request is refused`() = runTest {
        val h = ready(this, snap("n"))
        h.dismiss("n", idem = ByteString.copyFrom(ByteArray(3))).assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
        h.dismiss("n", idem = idempotency(5)) { createdUnixMs = h.now - 25L * 60 * 60 * 1000 }
            .assertError(ErrorCode.ERROR_CODE_EXPIRED)
        assertTrue(h.canceller.cancelled.isEmpty())
    }
}
