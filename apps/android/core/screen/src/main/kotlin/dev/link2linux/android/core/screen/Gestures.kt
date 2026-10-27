package dev.link2linux.android.core.screen

import dev.link2linux.protocol.v1.ScreenPoint
import kotlin.math.min

/**
 * The video a stream sends: its `ScreenFormat.id`, its frames' size, and
 * the size of the screen they show, onto which the desktop's points map.
 */
internal data class VideoFormat(val id: Int, val width: Int, val height: Int, val screenWidth: Int, val screenHeight: Int)

/** Checks the desktop's gestures and turns them into strokes on the screen (docs/protocol.md, "Control"). */
internal object Gestures {
    const val MAX_POINTS = 64
    const val MAX_MILLIS = 10_000L

    /** How long a tap, a gesture of one point, touches the screen. */
    const val TAP_MILLIS = 40L

    /**
     * The shortest rest that splits a touch. The desktop samples the mouse
     * every frame or so, and a hand that pauses for a frame or two while it
     * drags has not meant to stop.
     */
    const val MIN_REST_MILLIS = 100L

    /** Why [points] are out of range, apart from where they lie, or null when they are in range. */
    fun shapeError(points: List<ScreenPoint>): String? = when {
        points.isEmpty() -> "A gesture without points"
        points.size > MAX_POINTS -> "A gesture with more than $MAX_POINTS points"
        time(points[0]) != 0L -> "A gesture whose first time is not 0"
        points.zipWithNext().any { (a, b) -> time(b) < time(a) } -> "A gesture whose times go back"
        time(points.last()) > MAX_MILLIS -> "A gesture longer than $MAX_MILLIS ms"
        else -> null
    }

    /** Whether every point lies in the frames of [video]. */
    fun inFrame(points: List<ScreenPoint>, video: VideoFormat): Boolean =
        points.all { unsigned(it.x) < video.width && unsigned(it.y) < video.height }

    /**
     * The strokes that carry [points] out on the screen, mapped from the
     * frames of [video]. The touch is split where the finger rests at one
     * point for [MIN_REST_MILLIS] or more, and where it starts moving again,
     * so that a long press before a drag lasts as long as the desktop's user
     * held it. A shorter rest among moves only slows the move down, and
     * stays in its stroke. Points in range, as [shapeError] and [inFrame]
     * check.
     */
    fun strokes(points: List<ScreenPoint>, video: VideoFormat): List<Stroke> {
        val map = Mapping(video)
        val runs = ArrayList<Run>()
        for ((a, b) in points.zipWithNext()) {
            val rests = a.x == b.x && a.y == b.y
            // The same point at the same time adds nothing.
            if (rests && time(a) == time(b)) continue
            val last = runs.lastOrNull()
            if (last != null && last.rests == rests) {
                if (!rests) last.points += b
                last.endMs = time(b)
            } else {
                runs += Run(rests, if (rests) arrayListOf(a) else arrayListOf(a, b), time(a), time(b))
            }
        }
        if (runs.isEmpty()) return listOf(Stroke(listOf(map.onScreen(points[0])), TAP_MILLIS))
        val strokes = ArrayList<Run>()
        for (run in runs) {
            val rests = run.rests && (runs.size == 1 || run.endMs - run.startMs >= MIN_REST_MILLIS)
            val last = strokes.lastOrNull()
            if (last != null && !last.rests && !rests) {
                // Runs share the point where one ends and the next starts.
                last.points += run.points.drop(1)
                last.endMs = run.endMs
            } else {
                strokes += Run(rests, run.points.toMutableList(), run.startMs, run.endMs)
            }
        }
        return strokes.map { Stroke(it.points.map(map::onScreen), (it.endMs - it.startMs).coerceAtLeast(1)) }
    }

    /** A part of the touch where the finger moves along [points], or rests at the first. */
    private class Run(val rests: Boolean, val points: MutableList<ScreenPoint>, val startMs: Long, var endMs: Long)

    /**
     * From the frames' pixels to the screen's. The frames show the whole
     * screen, scaled to fit and centered: where alignment made a frame's
     * shape differ from the screen's, the edges left over are empty, and a
     * point there goes to the nearest edge of the screen.
     */
    private class Mapping(private val video: VideoFormat) {
        private val scale = min(video.width.toDouble() / video.screenWidth, video.height.toDouble() / video.screenHeight)
        private val left = (video.width - video.screenWidth * scale) / 2
        private val top = (video.height - video.screenHeight * scale) / 2

        fun onScreen(point: ScreenPoint): ScreenPointF = ScreenPointF(
            x = ((unsigned(point.x) + 0.5 - left) / scale).coerceIn(0.0, video.screenWidth - 1.0).toFloat(),
            y = ((unsigned(point.y) + 0.5 - top) / scale).coerceIn(0.0, video.screenHeight - 1.0).toFloat(),
        )
    }

    // The fields are uint32, which the Java classes hold in an Int.
    private fun unsigned(value: Int): Long = Integer.toUnsignedLong(value)

    private fun time(point: ScreenPoint): Long = unsigned(point.timeMs)
}
