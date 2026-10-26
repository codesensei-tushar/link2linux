package dev.link2linux.android.core.notifications

import dev.link2linux.android.core.notifications.EngineHarness.Companion.snap
import dev.link2linux.protocol.v1.ImageFormat
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** One slow image or one slow desktop must not hold up the rest. */
class NotificationEngineIsolationTest {

    /** Renders only when the test says so; a render it never finishes is one that never returns. */
    private class ManualRenderer : ImageRenderer {
        private val waiting = mutableListOf<() -> Unit>()

        override fun render(source: ImageSource, kind: ImageKind, done: (EncodedImage?) -> Unit) {
            waiting += { done(ImageRenderers.renderSafely(source, kind)) }
        }

        fun finishAll() {
            val now = waiting.toList()
            waiting.clear()
            now.forEach { it() }
        }
    }

    private class Picture(private val fill: Byte) : ImageSource {
        override fun render(kind: ImageKind) = EncodedImage(ImageFormat.IMAGE_FORMAT_PNG, ByteArray(64) { fill }, 8, 8)
    }

    @Test
    fun `a notification goes out at once while its picture is still loading, and the picture follows quietly`() = runTest {
        val renderer = ManualRenderer()
        val h = EngineHarness(this, images = ImageStore(renderer = renderer))
        h.connect()
        h.attach()
        h.sync()

        h.post(snap("a").copy(largeIcon = Picture(1)))
        h.post(snap("b"))
        h.remove("b")

        val first = h.transport.posted.first { it.notification.key == "a" }
        assertFalse(first.notification.hasLargeIcon())
        assertTrue(first.alert)
        assertEquals(listOf("b"), h.transport.removed.map { it.key })

        renderer.finishAll()
        h.idle()

        val update = h.transport.posted.last { it.notification.key == "a" }
        assertTrue(update.notification.hasLargeIcon())
        assertFalse(update.alert)
    }

    @Test
    fun `an image that runs out of memory leaves the notification without it`() = runTest {
        val h = EngineHarness(this)
        h.connect()
        h.attach()
        h.sync()

        val huge = object : ImageSource {
            override fun render(kind: ImageKind): EncodedImage = throw OutOfMemoryError("20000 x 20000")
        }
        h.post(snap("a").copy(largeIcon = huge, picture = huge))
        h.post(snap("b"))

        assertEquals(listOf("a", "b"), h.transport.posted.map { it.notification.key })
        assertFalse(h.transport.posted.first().notification.hasLargeIcon())
    }

    @Test
    fun `a desktop that stops reading holds up no other desktop`() = runTest {
        val h = EngineHarness(this)
        val stuck = FakeTransport().apply { stall = CompletableDeferred() }
        h.connect(snap("old"))
        h.attach()
        h.attach(stuck)
        h.sync()
        h.sync(from = stuck)

        h.post(snap("a"))
        h.post(snap("b"))
        val answer = h.dismiss("old")

        assertEquals(listOf("a", "b"), h.transport.posted.map { it.notification.key })
        assertEquals(NotificationReply.Ack, answer.single)
        assertTrue(stuck.events.isEmpty())

        stuck.stall!!.complete(Unit)
        h.idle()
        assertEquals(listOf("a", "b"), stuck.posted.map { it.notification.key })
    }

    @Test
    fun `a desktop that falls too far behind is ended so it can sync again`() = runTest {
        val h = EngineHarness(this, outboxCapacity = 4)
        val stuck = FakeTransport().apply { stall = CompletableDeferred() }
        h.connect()
        h.attach(stuck)
        h.sync(from = stuck)

        // One write in progress and four queued fit; the sixth does not.
        repeat(5) { h.post(snap("n$it")) }
        assertFalse(stuck.abandoned)
        h.post(snap("n5"))
        assertTrue(stuck.abandoned)
    }
}
