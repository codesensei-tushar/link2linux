package dev.link2linux.android.core.notifications

import dev.link2linux.android.core.notifications.EngineHarness.Companion.snap
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.imageRequest
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
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
    fun `a request the engine does not handle yet is unsupported`() = runTest {
        val sink = RecordingSink()
        EngineHarness(this).engine.handle(NotificationRequest.Image(imageRequest { }), sink)
        sink.assertError(ErrorCode.ERROR_CODE_UNSUPPORTED)
    }
}
