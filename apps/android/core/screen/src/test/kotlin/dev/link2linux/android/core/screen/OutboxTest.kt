package dev.link2linux.android.core.screen

import dev.link2linux.protocol.v1.ScreenFormat
import dev.link2linux.protocol.v1.ScreenFrame
import dev.link2linux.protocol.v1.screenFormat
import dev.link2linux.protocol.v1.screenFrame
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

class OutboxTest {

    private val outbox = Outbox(maxWaitingUs = 500_000)

    private fun format(id: Int, input: Boolean = false): ScreenFormat = screenFormat {
        this.id = id
        this.input = input
    }

    private fun frame(atMs: Long, key: Boolean = false): ScreenFrame = screenFrame {
        timestampUs = atMs * 1000
        keyFrame = key
    }

    /** Takes the next message as the sender does: format ids as "F<id>", frames as their time in ms, "K" for key frames. */
    private suspend fun take(): String {
        val item = checkNotNull(outbox.next())
        outbox.written()
        return describe(item)
    }

    private fun describe(item: Outbox.Item): String = when (item) {
        is Outbox.Item.Format -> "F${item.format.id}" + if (item.format.input) "+input" else ""
        is Outbox.Item.Frame -> (if (item.frame.keyFrame) "K" else "") + item.frame.timestampUs / 1000
    }

    private suspend fun takeAll(count: Int): List<String> = List(count) { take() }

    @Test
    fun messagesGoOutInOrder() = runTest {
        outbox.format(format(1), newVideo = true)
        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(0, key = true)))
        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(16)))
        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(33)))

        assertEquals(listOf("F1", "K0", "16", "33"), takeAll(4))
    }

    @Test
    fun aFrameThatWouldMakeTheWaitTooLongIsDroppedWithTheFramesAfterItUntilAKeyFrame() = runTest {
        outbox.format(format(1), newVideo = true)
        outbox.frame(frame(0, key = true))
        outbox.frame(frame(400))

        assertEquals(Outbox.Offer.DROPPED, outbox.frame(frame(501)))
        assertEquals(Outbox.Offer.WAITING, outbox.frame(frame(510)))
        assertEquals(listOf("F1", "K0", "400"), takeAll(3))
        // The wait is over, but a frame after a dropped one cannot be decoded.
        assertEquals(Outbox.Offer.WAITING, outbox.frame(frame(600)))
        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(700, key = true)))
        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(716)))

        assertEquals(listOf("K700", "716"), takeAll(2))
    }

    @Test
    fun theFrameBeingWrittenCountsAsWaiting() = runTest {
        outbox.format(format(1), newVideo = true)
        outbox.frame(frame(0, key = true))
        take()
        // The key frame is being written: taken, not yet written.
        assertEquals(Outbox.Item.Frame::class, checkNotNull(outbox.next())::class)

        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(500)))
        assertEquals(Outbox.Offer.DROPPED, outbox.frame(frame(501)))
    }

    @Test
    fun aKeyFrameTakesThePlaceOfTheFramesStillWaitingButNotOfFormats() = runTest {
        outbox.format(format(1), newVideo = true)
        outbox.frame(frame(0, key = true))
        outbox.frame(frame(16))
        outbox.format(format(1, input = true), newVideo = false)
        outbox.frame(frame(33))

        assertEquals(Outbox.Offer.QUEUED, outbox.frame(frame(50, key = true)))

        assertEquals(listOf("F1", "F1+input", "K50"), takeAll(3))
    }

    @Test
    fun aFormatOfNewVideoDropsWhatWaitsAndWaitsForItsKeyFrame() = runTest {
        outbox.format(format(1), newVideo = true)
        outbox.frame(frame(0, key = true))
        outbox.frame(frame(16))

        outbox.format(format(2), newVideo = true)
        assertEquals(Outbox.Offer.WAITING, outbox.frame(frame(33)))
        outbox.frame(frame(50, key = true))

        assertEquals(listOf("F2", "K50"), takeAll(2))
    }

    @Test
    fun skippingToAKeyFrameKeepsWhatAlreadyWaits() = runTest {
        outbox.format(format(1), newVideo = true)
        outbox.frame(frame(0, key = true))
        outbox.frame(frame(16))

        outbox.skipToKeyFrame()

        assertEquals(Outbox.Offer.WAITING, outbox.frame(frame(33)))
        assertEquals(listOf("F1", "K0", "16"), takeAll(3))
    }

    @Test
    fun nextWaitsForAMessage() = runTest {
        val next = async { outbox.next() }
        runCurrent()
        assertFalse(next.isCompleted)

        outbox.format(format(3), newVideo = true)

        assertEquals("F3", describe(checkNotNull(next.await())))
    }

    @Test
    fun closingEndsTheWaitAndDropsWhatWaits() = runTest {
        val next = async { outbox.next() }
        runCurrent()

        outbox.close()

        assertNull(next.await())
        outbox.format(format(1), newVideo = true)
        assertSame(Outbox.Offer.CLOSED, outbox.frame(frame(0, key = true)))
        assertNull(outbox.next())
    }
}
