package dev.link2linux.android.core.screen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PacingTest {

    @Test
    fun framesAt60HzAllGoOutAlsoWithJitter() {
        val rate = FrameRate()
        // 16.667 ms apart, each one up to 0.2 ms early or late.
        val sent = (0 until 600).count { rate.take(it * 16_667L + if (it % 2 == 0) 200 else -200) }

        assertEquals(600, sent)
    }

    @Test
    fun framesAt120HzAreHalved() {
        val rate = FrameRate()

        val sent = (0 until 1200).count { rate.take(it * 8_333L) }

        // Ten seconds' worth at 60 a second, and the one extra frame of the first burst.
        assertEquals(601, sent)
    }

    @Test
    fun qualityDropsAtOnceButAtMostOnceASecond() {
        val quality = Quality()

        assertEquals(0.7f, quality.lower(0)!!, 0.001f)
        assertNull(quality.lower(500_000))
        assertEquals(0.49f, quality.lower(1_000_000)!!, 0.001f)
    }

    @Test
    fun qualityNeverDropsBelowItsFloor() {
        val quality = Quality()

        repeat(20) { quality.lower(it * 1_000_000L) }

        assertEquals(Quality.MIN, quality.value)
        assertNull(quality.lower(30_000_000))
    }

    @Test
    fun qualityRecoversStepByStepWhileFramesGoOutFine() {
        val quality = Quality()
        quality.lower(0)

        assertNull(quality.fine(4_999_999))
        assertEquals(0.875f, quality.fine(5_000_000)!!, 0.001f)
        assertNull(quality.fine(9_000_000))
        assertEquals(1f, quality.fine(10_000_000)!!, 0.001f)
        assertNull(quality.fine(20_000_000))
    }
}
