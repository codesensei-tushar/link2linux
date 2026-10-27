package dev.link2linux.android.feature.screen

import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.media.Image
import android.media.ImageReader
import android.os.Handler
import android.util.Log
import android.view.Surface
import androidx.core.graphics.createBitmap
import com.google.protobuf.ByteString
import dev.link2linux.android.core.screen.FrameSize
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
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
        // An image can already be waiting if the display drew before the listener was set.
        take()
    }

    override fun requestKeyFrame() {
        val bitmap = last
        if (bitmap != null) send(bitmap, System.nanoTime() / 1000) else take()
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
        try {
            val wait = lastAtNanos + INTERVAL_NANOS - System.nanoTime()
            if (wait <= 0) return take()
            waiting = true
            handler.postDelayed({
                waiting = false
                if (!released) take()
            }, wait / 1_000_000 + 1)
        } catch (e: RuntimeException) {
            // A bad image must end the stream. Escaping here kills the capture thread and the
            // stream never hears that it stopped.
            Log.w(TAG, "Could not read a screen image: ${e.javaClass.simpleName}")
            if (!released) events.failed()
        }
    }

    private fun take() {
        val image = try {
            reader.acquireLatestImage()
        } catch (_: IllegalStateException) {
            null
        } ?: return
        val (bitmap, timestampUs) = try {
            image.use { it.toBitmap() to it.timestamp / 1000 }
        } catch (e: RuntimeException) {
            Log.w(TAG, "Could not read a screen image: ${e.javaClass.simpleName}")
            if (!released) events.failed()
            return
        }
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

    /**
     * The image's pixels. Rows may be longer than the image, and the last row may not carry
     * that padding, so the bytes are copied a row at a time. Copying a padded bitmap in one
     * go reads past the buffer and fails the capture thread.
     */
    private fun Image.toBitmap(): Bitmap {
        val plane = planes[0]
        val pixelStride = plane.pixelStride
        val rowStride = plane.rowStride
        val src = plane.buffer.duplicate()
        val bitmap = createBitmap(width, height)
        try {
            if (pixelStride == 4 && rowStride == width * 4 && src.remaining() >= width * height * 4) {
                bitmap.copyPixelsFromBuffer(src)
                return bitmap
            }
            if (pixelStride != 4) throw IllegalArgumentException("A screen image with pixel stride $pixelStride")
            val packed = ByteBuffer.allocate(width * height * 4)
            val base = src.position()
            val limit = src.limit()
            val rowBytes = width * 4
            val rowBuf = src.duplicate()
            for (y in 0 until height) {
                val row = base + y * rowStride
                val have = if (row < limit) minOf(rowBytes, limit - row) else 0
                if (have > 0) {
                    rowBuf.limit(limit)
                    rowBuf.position(row)
                    rowBuf.limit(row + have)
                    packed.put(rowBuf)
                }
                repeat(rowBytes - have) { packed.put(0.toByte()) }
            }
            packed.flip()
            bitmap.copyPixelsFromBuffer(packed)
            return bitmap
        } catch (e: RuntimeException) {
            bitmap.recycle()
            throw e
        }
    }

    companion object {
        /** The longest side when the desktop leaves it to the phone: JPEG frames grow fast with the size. */
        private const val DEFAULT_MAX_SIZE = 1280
        private const val FRAMES_PER_SECOND = 12
        private const val INTERVAL_NANOS = 1_000_000_000L / FRAMES_PER_SECOND
        private const val TAG = "link2linux"
        private const val FULL_QUALITY = 70
        private const val MIN_QUALITY = 20

        fun create(screen: ScreenSize, maxSize: Int, handler: Handler, events: EncoderEvents): JpegEncoder? {
            val size = FrameSize.fit(screen.width, screen.height, maxSize, DEFAULT_MAX_SIZE) ?: return null
            return JpegEncoder(size, handler, events)
        }
    }
}
