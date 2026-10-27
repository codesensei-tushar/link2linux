package dev.link2linux.android.feature.screen

import android.content.Context
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.projection.MediaProjection
import android.os.Handler
import android.os.HandlerThread
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.Surface
import com.google.protobuf.ByteString
import dev.link2linux.android.core.screen.CaptureConfig
import dev.link2linux.android.core.screen.CaptureFormat
import dev.link2linux.android.core.screen.CaptureSink
import dev.link2linux.android.core.screen.ScreenCapture
import dev.link2linux.protocol.v1.ScreenCodec
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit

/** The screen's size in pixels in its current orientation, and its density. */
internal data class ScreenSize(val width: Int, val height: Int, val dpi: Int)

/** What an encoder tells the capture, on the capture's thread. */
internal interface EncoderEvents {
    fun format(codecConfig: ByteString, width: Int, height: Int)

    fun frame(timestampUs: Long, keyFrame: Boolean, data: ByteArray)

    fun failed()
}

/** Turns what the virtual display draws on [surface] into frames, at [width] by [height]. */
internal interface FrameEncoder {
    val width: Int
    val height: Int
    val surface: Surface

    /** Starts encoding; the format comes first, then a key frame. */
    fun start()

    fun requestKeyFrame()

    fun setQuality(quality: Float)

    fun release()
}

/**
 * One media projection that the user agreed to, captured into a virtual
 * display that mirrors the screen and drawn into an encoder: H.264 through
 * the phone's encoder, or JPEG through an image reader.
 *
 * Everything runs on a thread of its own. Android 14 lets an app create
 * one virtual display per projection, so when the screen rotates or the
 * desktop asks again, a new encoder takes the display's surface and the
 * display is resized; frames of the encoder it replaced are dropped. The
 * projection's own stop, by the user or because the phone locked, ends the
 * stream.
 */
internal class AndroidScreenCapture(
    context: Context,
    private val projection: MediaProjection,
    private val onReleased: () -> Unit,
) : ScreenCapture {
    private val displays = context.getSystemService(DisplayManager::class.java)
    private val thread = HandlerThread("link2linux-screen").apply { start() }
    private val handler = Handler(thread.looper)

    // Touched only on the capture's thread.
    private var sink: CaptureSink? = null
    private var config: CaptureConfig? = null
    private var display: VirtualDisplay? = null
    private var encoder: FrameEncoder? = null
    private var screen: ScreenSize? = null
    private var generation = 0
    private var quality = 1f
    private var released = false

    private val projectionCallback = object : MediaProjection.Callback() {
        override fun onStop() {
            if (!released) sink?.onStopped()
        }

        // Android 14 and later: the captured content changed its size, as when the phone rotates.
        override fun onCapturedContentResize(width: Int, height: Int) {
            if (!released) follow()
        }
    }

    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) = Unit

        override fun onDisplayRemoved(displayId: Int) = Unit

        override fun onDisplayChanged(displayId: Int) {
            if (displayId == Display.DEFAULT_DISPLAY && !released) follow()
        }
    }

    override fun start(config: CaptureConfig, sink: CaptureSink): Boolean = onThread(fallback = false) {
        this.config = config
        this.sink = sink
        // Before the virtual display, which Android 14 requires.
        projection.registerCallback(projectionCallback, handler)
        displays?.registerDisplayListener(displayListener, handler)
        encode()
    }

    override fun reconfigure(config: CaptureConfig) {
        handler.post {
            if (released) return@post
            this.config = config
            if (!encode()) sink?.onFailed()
        }
    }

    override fun requestKeyFrame() {
        handler.post { encoder?.requestKeyFrame() }
    }

    override fun setQuality(quality: Float) {
        handler.post {
            this.quality = quality
            encoder?.setQuality(quality)
        }
    }

    override fun stop() {
        handler.post { release() }
    }

    /** Encodes the screen as it is now with a new encoder, which takes the virtual display's surface. */
    private fun encode(): Boolean {
        val config = config ?: return false
        val screen = currentScreen() ?: return false
        val events = Events(++generation, config.codec, screen)
        val next = when (config.codec) {
            ScreenCodec.SCREEN_CODEC_H264 -> AvcEncoder.create(screen, config.maxSize, handler, events)
            ScreenCodec.SCREEN_CODEC_JPEG -> JpegEncoder.create(screen, config.maxSize, handler, events)
            else -> null
        } ?: return false
        next.setQuality(quality)
        // Start before the display draws into the surface. A picture that arrives first is lost:
        // the encoder's listener is not told about an image already waiting, and a screen that
        // does not change again never sends a frame.
        try {
            next.start()
            val current = display
            if (current == null) {
                display = projection.createVirtualDisplay(
                    "link2linux",
                    next.width,
                    next.height,
                    screen.dpi,
                    DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                    next.surface,
                    null,
                    handler,
                ) ?: throw IllegalStateException("No virtual display")
            } else {
                current.surface = next.surface
                current.resize(next.width, next.height, screen.dpi)
            }
        } catch (e: RuntimeException) {
            Log.w(TAG, "Could not capture the screen: ${e.javaClass.simpleName}")
            next.release()
            return false
        }
        encoder?.release()
        encoder = next
        this.screen = screen
        return true
    }

    /** The screen changed: a new size, as after a rotation, needs a new encoder. */
    private fun follow() {
        val now = currentScreen() ?: return
        if (now == screen) return
        if (!encode()) sink?.onFailed()
    }

    private fun currentScreen(): ScreenSize? {
        val display = displays?.getDisplay(Display.DEFAULT_DISPLAY) ?: return null
        val metrics = DisplayMetrics()
        // The whole screen in its current orientation, which is what the projection mirrors.
        @Suppress("DEPRECATION")
        display.getRealMetrics(metrics)
        return ScreenSize(metrics.widthPixels, metrics.heightPixels, metrics.densityDpi).takeIf { it.width > 0 && it.height > 0 }
    }

    private fun release() {
        if (released) return
        released = true
        displays?.unregisterDisplayListener(displayListener)
        projection.unregisterCallback(projectionCallback)
        display?.release()
        display = null
        encoder?.release()
        encoder = null
        projection.stop()
        thread.quitSafely()
        onReleased()
    }

    /** Runs [block] on the capture's thread and waits for it, or gives [fallback] when it cannot. */
    private fun <T> onThread(fallback: T, block: () -> T): T {
        val task = FutureTask(block)
        if (!handler.post(task)) return fallback
        return try {
            task.get(START_SECONDS, TimeUnit.SECONDS)
        } catch (e: Exception) {
            Log.w(TAG, "Could not start the screen capture: ${e.javaClass.simpleName}")
            fallback
        }
    }

    /** One encoder's events, passed on only while it is the current one. */
    private inner class Events(private val generation: Int, private val codec: ScreenCodec, private val screen: ScreenSize) : EncoderEvents {
        private val current: Boolean get() = !released && generation == this@AndroidScreenCapture.generation

        override fun format(codecConfig: ByteString, width: Int, height: Int) {
            if (current) sink?.onFormat(CaptureFormat(codec, width, height, codecConfig, screen.width, screen.height))
        }

        override fun frame(timestampUs: Long, keyFrame: Boolean, data: ByteArray) {
            if (current) sink?.onFrame(timestampUs, keyFrame, data)
        }

        override fun failed() {
            if (current) sink?.onFailed()
        }
    }

    companion object {
        private const val TAG = "link2linux"
        private const val START_SECONDS = 5L

        /** The codecs this phone can encode: H.264 where it has an encoder, and JPEG always. */
        fun encodable(): Set<ScreenCodec> = buildSet {
            if (AvcEncoder.find() != null) add(ScreenCodec.SCREEN_CODEC_H264)
            add(ScreenCodec.SCREEN_CODEC_JPEG)
        }
    }
}
