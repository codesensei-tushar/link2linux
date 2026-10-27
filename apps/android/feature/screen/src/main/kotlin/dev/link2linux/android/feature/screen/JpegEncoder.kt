package dev.link2linux.android.feature.screen

import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.media.Image
import android.media.ImageReader
import android.os.Handler
import android.view.Surface
import androidx.core.graphics.createBitmap
import com.google.protobuf.ByteString
import dev.link2linux.android.core.screen.FrameSize
import java.io.ByteArrayOutputStream
import kotlin.math.roundToInt

/**
 * JPEG frames for desktops without an H.264 decoder: the virtual display
 * draws into an image reader, and each image becomes one JPEG, a key frame
 * of its own. At most [FRAMES_PER_SECOND] a second: an image that comes
 * sooner waits for its turn, and a newer one takes its place, so the last
 * picture of a change is always sent. The last picture is kept, to send
 * again when the desktop asks for a key frame while the screen does not
 * change.
 */
internal class JpegEncoder private constructor(
    size: FrameSize,
    private val handler: Handler,
    private val events: EncoderEvents,
) : FrameEncoder {
    override val width = size.width
    override val height = size.height
    private val reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2)
    override val surface: Surface = reader.surface

    // Touched only on the capture's thread.
    private var last: Bitmap? = null
    private var lastAtNanos = 0L
    private var waiting = false
    private var released = false
    private var jpegQuality = FULL_QUALITY

    override fun start() {
        events.format(ByteString.EMPTY, width, height)
        reader.setOnImageAvailableListener({ onImage() }, handler)
    }

    override fun requestKeyFrame() {
        last?.let { send(it, System.nanoTime() / 1000) }
    }

    override fun setQuality(quality: Float) {
        jpegQuality = (FULL_QUALITY * quality).roundToInt().coerceAtLeast(MIN_QUALITY)
    }

    override fun release() {
        if (released) return
        released = true
        reader.close()
        last?.recycle()
        last = null
    }

    private fun onImage() {
        if (released || waiting) return
        val wait = lastAtNanos + INTERVAL_NANOS - System.nanoTime()
        if (wait <= 0) return take()
        waiting = true
        handler.postDelayed({
            waiting = false
            if (!released) take()
        }, wait / 1_000_000 + 1)
    }

    private fun take() {
        val image = try {
            reader.acquireLatestImage()
        } catch (_: IllegalStateException) {
            null
        } ?: return
        val (bitmap, timestampUs) = image.use { it.toBitmap() to it.timestamp / 1000 }
        last?.recycle()
        last = bitmap
        lastAtNanos = System.nanoTime()
        send(bitmap, timestampUs)
    }

    private fun send(bitmap: Bitmap, timestampUs: Long) {
        val out = ByteArrayOutputStream()
        if (!bitmap.compress(Bitmap.CompressFormat.JPEG, jpegQuality, out)) return events.failed()
        events.frame(timestampUs, true, out.toByteArray())
    }

    /** The image's pixels; rows may carry padding past the image's width, which is cut off. */
    private fun Image.toBitmap(): Bitmap {
        val plane = planes[0]
        val rowPixels = plane.rowStride / plane.pixelStride
        val padded = createBitmap(rowPixels, height)
        padded.copyPixelsFromBuffer(plane.buffer)
        if (rowPixels == width) return padded
        return Bitmap.createBitmap(padded, 0, 0, width, height).also { padded.recycle() }
    }

    companion object {
        /** The longest side when the desktop leaves it to the phone: JPEG frames grow fast with the size. */
        private const val DEFAULT_MAX_SIZE = 1280
        private const val FRAMES_PER_SECOND = 12
        private const val INTERVAL_NANOS = 1_000_000_000L / FRAMES_PER_SECOND
        private const val FULL_QUALITY = 70
        private const val MIN_QUALITY = 20

        fun create(screen: ScreenSize, maxSize: Int, handler: Handler, events: EncoderEvents): JpegEncoder? {
            val size = FrameSize.fit(screen.width, screen.height, maxSize, DEFAULT_MAX_SIZE) ?: return null
            return JpegEncoder(size, handler, events)
        }
    }
}
