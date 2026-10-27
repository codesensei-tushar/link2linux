package dev.link2linux.android.core.screen

import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** A frame's size in pixels. */
data class FrameSize(val width: Int, val height: Int) {

    companion object {
        /** The smallest longer side worth sending; below it the frames show nothing readable. */
        const val MIN_LONG_SIDE = 160

        /**
         * The size of the frames for a [screenWidth] by [screenHeight] screen
         * (docs/protocol.md, "Video"): the screen's shape, scaled down so
         * that the longer side is at most [maxSize], or [defaultMax] when
         * [maxSize] is 0, and never scaled up. Each side is a multiple of its
         * alignment, which is even, as encoders want; when [supported]
         * rejects a size, the next one is a tenth smaller. Null when no size
         * from [MIN_LONG_SIDE] up works.
         */
        fun fit(
            screenWidth: Int,
            screenHeight: Int,
            maxSize: Int,
            defaultMax: Int,
            widthAlignment: Int = 2,
            heightAlignment: Int = 2,
            supported: (width: Int, height: Int) -> Boolean = { _, _ -> true },
        ): FrameSize? {
            if (screenWidth <= 0 || screenHeight <= 0) return null
            val wAlign = evenAlignment(widthAlignment)
            val hAlign = evenAlignment(heightAlignment)
            val longSide = max(screenWidth, screenHeight)
            var target = min(longSide, if (maxSize == 0) defaultMax else maxSize)
            while (target >= MIN_LONG_SIDE) {
                val scale = target.toDouble() / longSide
                val width = alignDown((screenWidth * scale).roundToInt(), wAlign)
                val height = alignDown((screenHeight * scale).roundToInt(), hAlign)
                if (width > 0 && height > 0 && max(width, height) <= target && supported(width, height)) {
                    return FrameSize(width, height)
                }
                target = (target * 0.9).toInt()
            }
            return null
        }

        private fun evenAlignment(alignment: Int): Int = if (alignment <= 2) 2 else alignment + alignment % 2

        private fun alignDown(value: Int, alignment: Int): Int = value / alignment * alignment
    }
}
