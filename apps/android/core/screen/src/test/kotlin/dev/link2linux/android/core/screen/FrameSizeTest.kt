package dev.link2linux.android.core.screen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FrameSizeTest {

    @Test
    fun theLongerSideIsScaledToTheDefaultKeepingTheShape() {
        assertEquals(FrameSize(864, 1920), FrameSize.fit(1080, 2400, maxSize = 0, defaultMax = 1920))
        assertEquals(FrameSize(1920, 864), FrameSize.fit(2400, 1080, maxSize = 0, defaultMax = 1920))
    }

    @Test
    fun theDesktopsMaxSizeWinsOverTheDefault() {
        assertEquals(FrameSize(540, 1200), FrameSize.fit(1080, 2400, maxSize = 1200, defaultMax = 1920))
    }

    @Test
    fun theScreenIsNeverScaledUp() {
        assertEquals(FrameSize(1080, 2400), FrameSize.fit(1080, 2400, maxSize = 4096, defaultMax = 1920))
    }

    @Test
    fun sidesAreEvenAndFollowTheEncodersAlignment() {
        // 1080 x 2340 at 1920 is 886.15 wide: 886 even, 880 on 16.
        assertEquals(FrameSize(886, 1920), FrameSize.fit(1080, 2340, maxSize = 0, defaultMax = 1920))
        assertEquals(FrameSize(880, 1920), FrameSize.fit(1080, 2340, maxSize = 0, defaultMax = 1920, 16, 16))
        // An odd screen side still comes out even.
        assertEquals(FrameSize(240, 534), FrameSize.fit(241, 535, maxSize = 0, defaultMax = 1920, widthAlignment = 1, heightAlignment = 1))
    }

    @Test
    fun aSizeTheEncoderRejectsGivesWayToSmallerOnes() {
        val size = FrameSize.fit(1080, 2400, maxSize = 0, defaultMax = 1920) { _, height -> height <= 1088 }

        assertEquals(FrameSize(458, 1018), size)
    }

    @Test
    fun withoutAnySizeTheEncoderTakesThereIsNone() {
        assertNull(FrameSize.fit(1080, 2400, maxSize = 0, defaultMax = 1920) { _, _ -> false })
        assertNull(FrameSize.fit(0, 2400, maxSize = 0, defaultMax = 1920))
    }
}
