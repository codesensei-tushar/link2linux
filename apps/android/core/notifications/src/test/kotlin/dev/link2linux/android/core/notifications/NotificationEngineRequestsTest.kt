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
    fun `a notification hidden for its one-time code can still be dismissed but not acted on`() = runTest {
        val rec = Recorder()
        val h = ready(this, snap("n", text = "Your code is 482913").copy(actions = listOf(ActionSnapshot("Copy", invoker = rec))))
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, h.act("n", "a:any").errorCode)
        assertTrue(rec.calls.isEmpty())
        h.dismiss("n").also { assertEquals(NotificationReply.Ack, it.single) }
        assertEquals(listOf("n"), h.canceller.cancelled)
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

    // Actions

    @Test
    fun `an action fires and is acknowledged`() = runTest {
        val rec = Recorder()
        val h = ready(this, withAction(ActionSnapshot("Archive", AndroidValues.SEMANTIC_ARCHIVE, invoker = rec)))
        val id = actionId(h)
        assertEquals(NotificationReply.Ack, h.act("n", id).single)
        assertEquals(listOf<Pair<String?, Boolean>>(null to false), rec.calls)
    }

    @Test
    fun `an unknown notification or action is not found`() = runTest {
        val h = ready(this, withAction(ActionSnapshot("Archive", invoker = Recorder())))
        h.act("zzz", "a").assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
        h.act("n", "zzz", idem = idempotency(3)).assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
    }

    @Test
    fun `a reply sends its text to the action`() = runTest {
        val rec = Recorder()
        val h = ready(
            this,
            withAction(ActionSnapshot("Reply", AndroidValues.SEMANTIC_REPLY, "r", ReplyInput(), invoker = rec)),
        )
        val id = actionId(h)
        assertEquals(NotificationReply.Ack, h.act("n", id, reply = "On my way").single)
        assertEquals(listOf<Pair<String?, Boolean>>("On my way" to false), rec.calls)
    }

    @Test
    fun `reply text rules`() = runTest {
        val reply = Recorder()
        val plain = Recorder()
        val h = ready(
            this,
            snap("n").copy(
                actions = listOf(
                    ActionSnapshot("Reply", replyKey = "r", reply = ReplyInput(allowFreeForm = false, choices = listOf("Yes", "No")), invoker = reply),
                    ActionSnapshot("Open", invoker = plain),
                ),
            ),
        )
        val replyId = actionId(h, index = 0)
        val plainId = actionId(h, index = 1)
        h.act("n", replyId, reply = "", idem = idempotency(1)).assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
        h.act("n", replyId, reply = "Maybe", idem = idempotency(2)).assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
        h.act("n", replyId, reply = "x".repeat(9000), idem = idempotency(3)).assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
        h.act("n", plainId, reply = "hi", idem = idempotency(4)).assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
        assertTrue(reply.calls.isEmpty() && plain.calls.isEmpty())
        assertEquals(NotificationReply.Ack, h.act("n", replyId, reply = "Yes", idem = idempotency(5)).single)
        assertEquals("Yes", reply.calls.single().first)
    }

    @Test
    fun `an action that needs unlock is not allowed while the phone is locked`() = runTest {
        val rec = Recorder()
        val h = ready(this, withAction(ActionSnapshot("Pay", requiresUnlock = true, invoker = rec)))
        val id = actionId(h)
        h.env.isLocked = true
        h.act("n", id).assertError(ErrorCode.ERROR_CODE_NOT_ALLOWED)
        assertTrue(rec.calls.isEmpty())
        h.env.isLocked = false
        assertEquals(NotificationReply.Ack, h.act("n", id, idem = idempotency(7)).single)
        assertEquals(1, rec.calls.size)
    }

    @Test
    fun `a locked phone does not block actions that do not need unlock`() = runTest {
        val rec = Recorder()
        val h = ready(this, withAction(ActionSnapshot("Mark read", invoker = rec)))
        h.env.isLocked = true
        assertEquals(NotificationReply.Ack, h.act("n", actionId(h)).single)
    }

    @Test
    fun `an action that opens an activity is not allowed from the background`() = runTest {
        val rec = Recorder()
        val h = ready(this, withAction(ActionSnapshot("Open", opensActivity = true, invoker = rec)))
        val id = actionId(h)
        h.act("n", id).assertError(ErrorCode.ERROR_CODE_NOT_ALLOWED)
        assertTrue(rec.calls.isEmpty())
        h.env.canStartActivitiesFromBackground = true
        assertEquals(NotificationReply.Ack, h.act("n", id, idem = idempotency(8)).single)
        assertEquals(listOf<Pair<String?, Boolean>>(null to true), rec.calls)
    }

    @Test
    fun `a cancelled pending intent means the action is gone`() = runTest {
        val h = ready(this, withAction(ActionSnapshot("Old", invoker = Recorder(ActionOutcome.CANCELED))))
        h.act("n", actionId(h)).assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
    }

    @Test
    fun `a system refusal is not allowed and a crash is internal`() = runTest {
        val blocked = ready(this, withAction(ActionSnapshot("X", invoker = Recorder(ActionOutcome.BLOCKED))))
        blocked.act("n", actionId(blocked)).assertError(ErrorCode.ERROR_CODE_NOT_ALLOWED)
        val broken = ready(this, withAction(ActionSnapshot("X", invoker = { _, _ -> error("boom") })))
        broken.act("n", actionId(broken)).assertError(ErrorCode.ERROR_CODE_INTERNAL)
        assertEquals(1, broken.errors.size)
    }

    @Test
    fun `an action id from an older version of a notification still finds its action after an update`() = runTest {
        val rec = Recorder()
        val a = ActionSnapshot("Mark read", AndroidValues.SEMANTIC_MARK_AS_READ, invoker = rec)
        val b = ActionSnapshot("Mute", AndroidValues.SEMANTIC_MUTE, invoker = Recorder())
        val h = ready(this, snap("n").copy(actions = listOf(a, b)))
        val id = actionId(h, index = 0)
        h.post(snap("n", text = "v2").copy(actions = listOf(b, a)))
        assertEquals(NotificationReply.Ack, h.act("n", id).single)
        assertEquals(1, rec.calls.size)
    }

    @Test
    fun `a call notification offers answer and decline that run`() = runTest {
        val answer = Recorder()
        val h = ready(
            this,
            snap("call").copy(
                category = "call",
                actions = listOf(
                    ActionSnapshot("", callRole = CallRole.ANSWER, invoker = answer),
                    ActionSnapshot("", callRole = CallRole.DECLINE, invoker = Recorder()),
                ),
            ),
        )
        assertEquals(NotificationReply.Ack, h.act("call", "call:answer").single)
        assertEquals(1, answer.calls.size)
    }

    // Images

    private class Png(val seed: Int) : ImageSource {
        override fun render(kind: ImageKind) =
            EncodedImage(ImageFormat.IMAGE_FORMAT_PNG, ByteArray(20) { (it + seed).toByte() }, 8, 8)
    }

    @Test
    fun `an image of a shared notification is served by its hash`() = runTest {
        val h = ready(this, snap("n").copy(appIcon = Png(1)))
        val ref = (h.sync().replies.first() as NotificationReply.Batch).message.getNotifications(0).appIcon
        val sink = h.image(ref.sha256)
        val data = (sink.single as NotificationReply.Image).message
        assertArrayEquals(WireText.sha256(data.data.toByteArray()), ref.sha256.toByteArray())
    }

    @Test
    fun `an unknown image is not found and a short hash is invalid`() = runTest {
        val h = ready(this)
        h.image(ByteString.copyFrom(ByteArray(32))).assertError(ErrorCode.ERROR_CODE_NOT_FOUND)
        h.image(ByteString.copyFrom(ByteArray(5))).assertError(ErrorCode.ERROR_CODE_INVALID_ARGUMENT)
    }
}
