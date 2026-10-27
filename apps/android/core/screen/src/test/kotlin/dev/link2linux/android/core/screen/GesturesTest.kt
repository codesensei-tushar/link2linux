package dev.link2linux.android.core.screen

import dev.link2linux.protocol.v1.ScreenPoint
import dev.link2linux.protocol.v1.screenPoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class GesturesTest {

    // Frames at half the screen's size: frame pixel x shows screen pixels 2x and 2x + 1.
    private val video = VideoFormat(id = 1, width = 540, height = 1200, screenWidth = 1080, screenHeight = 2400)

    private fun point(x: Int, y: Int, timeMs: Int = 0): ScreenPoint = screenPoint {
        this.x = x
        this.y = y
        this.timeMs = timeMs
    }

    private fun at(x: Float, y: Float) = ScreenPointF(x, y)

    @Test
    fun pointsOutOfRangeAreRefused() {
        assertNotNull(Gestures.shapeError(emptyList()))
        assertNotNull(Gestures.shapeError(List(65) { point(1, 1, it) }))
        assertNotNull(Gestures.shapeError(listOf(point(1, 1, 5))))
        assertNotNull(Gestures.shapeError(listOf(point(1, 1), point(2, 2, 100), point(3, 3, 50))))
        assertNotNull(Gestures.shapeError(listOf(point(1, 1), point(2, 2, 10_001))))
        // A uint32 above Int.MAX_VALUE reads as negative in Java, and must not pass as small.
        assertNotNull(Gestures.shapeError(listOf(point(1, 1), point(2, 2, -1))))
    }

    @Test
    fun pointsInRangePass() {
        assertNull(Gestures.shapeError(listOf(point(1, 1))))
        assertNull(Gestures.shapeError(List(64) { point(it, it, it * 100) }))
        assertNull(Gestures.shapeError(listOf(point(1, 1), point(1, 1), point(2, 2, 10_000))))
    }

    @Test
    fun pointsMustLieInTheFrame() {
        assertTrue(Gestures.inFrame(listOf(point(0, 0), point(539, 1199)), video))
        assertFalse(Gestures.inFrame(listOf(point(540, 0)), video))
        assertFalse(Gestures.inFrame(listOf(point(0, 1200)), video))
        assertFalse(Gestures.inFrame(listOf(point(-1, 0)), video))
    }

    @Test
    fun onePointIsATapAtTheMiddleOfItsPixel() {
        assertEquals(listOf(Stroke(listOf(at(1f, 3f)), Gestures.TAP_MILLIS)), Gestures.strokes(listOf(point(0, 1)), video))
        // The same point again at the same time is still a tap.
        assertEquals(listOf(Stroke(listOf(at(1f, 3f)), Gestures.TAP_MILLIS)), Gestures.strokes(listOf(point(0, 1), point(0, 1)), video))
    }

    @Test
    fun aSwipeIsOneStrokeAlongItsPoints() {
        val strokes = Gestures.strokes(listOf(point(10, 100), point(10, 50, 100), point(10, 0, 250)), video)

        assertEquals(listOf(Stroke(listOf(at(21f, 201f), at(21f, 101f), at(21f, 1f)), 250)), strokes)
    }

    @Test
    fun aFingerThatStaysIsALongPress() {
        assertEquals(listOf(Stroke(listOf(at(21f, 21f)), 600)), Gestures.strokes(listOf(point(10, 10), point(10, 10, 600)), video))
    }

    @Test
    fun aLongPressBeforeADragKeepsItsLength() {
        val strokes = Gestures.strokes(listOf(point(10, 10), point(10, 10, 600), point(20, 10, 700), point(30, 10, 900)), video)

        assertEquals(
            listOf(
                Stroke(listOf(at(21f, 21f)), 600),
                Stroke(listOf(at(21f, 21f), at(41f, 21f), at(61f, 21f)), 300),
            ),
            strokes,
        )
    }

    @Test
    fun aRestAfterAMoveIsAStrokeOfItsOwn() {
        val strokes = Gestures.strokes(listOf(point(10, 10), point(20, 10, 100), point(20, 10, 800)), video)

        assertEquals(listOf(Stroke(listOf(at(21f, 21f), at(41f, 21f)), 100), Stroke(listOf(at(41f, 21f)), 700)), strokes)
    }

    @Test
    fun aShortPauseWhileDraggingStaysInTheStroke() {
        val strokes = Gestures.strokes(listOf(point(10, 10), point(20, 10, 50), point(20, 10, 149), point(30, 10, 200)), video)

        assertEquals(listOf(Stroke(listOf(at(21f, 21f), at(41f, 21f), at(61f, 21f)), 200)), strokes)
    }

    @Test
    fun aShortRestBeforeOrAfterAMoveIsPartOfIt() {
        assertEquals(
            listOf(Stroke(listOf(at(21f, 21f), at(41f, 21f)), 150)),
            Gestures.strokes(listOf(point(10, 10), point(10, 10, 50), point(20, 10, 100), point(20, 10, 150)), video),
        )
    }

    @Test
    fun aShortPressAloneIsAPressOfItsLength() {
        assertEquals(listOf(Stroke(listOf(at(21f, 21f)), 80)), Gestures.strokes(listOf(point(10, 10), point(10, 10, 80)), video))
    }

    @Test
    fun aRestOfTheShortestLengthSplitsTheTouch() {
        val strokes = Gestures.strokes(listOf(point(10, 10), point(20, 10, 50), point(20, 10, 150), point(30, 10, 200)), video)

        assertEquals(3, strokes.size)
        assertEquals(listOf(50L, Gestures.MIN_REST_MILLIS, 50L), strokes.map { it.durationMillis })
    }

    @Test
    fun aMoveWithoutTimeStillTakesAMoment() {
        assertEquals(listOf(Stroke(listOf(at(21f, 21f), at(41f, 21f)), 1)), Gestures.strokes(listOf(point(10, 10), point(20, 10)), video))
    }

    @Test
    fun theEmptyEdgesThatAlignmentLeftMapToTheScreensEdge() {
        // A 1080 x 2340 screen in 880 x 1920 frames: aligned to 16, the frames are narrower than the screen's
        // shape, so the picture fills their width and leaves a few empty rows at the top and the bottom.
        val aligned = VideoFormat(id = 1, width = 880, height = 1920, screenWidth = 1080, screenHeight = 2340)
        val scale = 880.0 / 1080
        val top = (1920 - 2340 * scale) / 2

        val strokes = Gestures.strokes(listOf(point(0, 0), point(879, 1919, 100)), aligned)

        val (first, last) = strokes.single().points
        assertEquals((0.5 / scale).toFloat(), first.x, 0.01f)
        assertEquals(0f, first.y, 0.01f)
        assertEquals((879.5 / scale).coerceAtMost(1079.0).toFloat(), last.x, 0.01f)
        assertEquals(((1919.5 - top) / scale).coerceAtMost(2339.0).toFloat(), last.y, 0.01f)
    }
}
