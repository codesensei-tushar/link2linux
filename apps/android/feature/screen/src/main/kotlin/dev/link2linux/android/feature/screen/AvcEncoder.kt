package dev.link2linux.android.feature.screen

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.util.Log
import android.view.Surface
import com.google.protobuf.ByteString
import dev.link2linux.android.core.screen.AvcStream
import dev.link2linux.android.core.screen.FrameSize
import java.io.IOException
import java.nio.ByteBuffer

/**
 * H.264 through the phone's encoder, fed by the virtual display's surface
 * (docs/protocol.md, "Video"). It asks for the constrained baseline
 * profile, which every decoder takes: no B-frames, so each frame is shown
 * as it arrives. For low latency it encodes at most 60 frames a second, a
 * key frame every few seconds and whenever asked, and repeats the last
 * frame while the screen does not change, so that a key frame asked for
 * comes even then.
 *
 * Its output is turned into what the protocol carries: the avcC record
 * from the encoder's parameter sets, and frames with 4-byte lengths in
 * front of their NAL units.
 */
internal class AvcEncoder private constructor(
    info: MediaCodecInfo,
    size: FrameSize,
    private val bitrate: Int,
    handler: Handler,
    private val events: EncoderEvents,
) : FrameEncoder {
    private val codec = MediaCodec.createByCodecName(info.name)
    override val width = size.width
    override val height = size.height
    override val surface: Surface

    // Touched only on the capture's thread, where the codec calls back.
    private var formatSent = false
    private var released = false

    init {
        try {
            codec.setCallback(Callback(), handler)
            codec.configure(formatFor(info, size, bitrate), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            surface = codec.createInputSurface()
        } catch (e: RuntimeException) {
            codec.release()
            throw e
        }
    }

    override fun start() = codec.start()

    override fun requestKeyFrame() = setParameters { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }

    override fun setQuality(quality: Float) =
        setParameters { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, (bitrate * quality).toInt().coerceAtLeast(MIN_BITRATE)) }

    override fun release() {
        if (released) return
        released = true
        try {
            codec.stop()
        } catch (_: IllegalStateException) {
            // Never started, or failed: releasing is all that is left.
        }
        codec.release()
        surface.release()
    }

    private fun setParameters(fill: Bundle.() -> Unit) {
        if (released) return
        try {
            codec.setParameters(Bundle().apply(fill))
        } catch (_: IllegalStateException) {
            // The codec failed; onError ends the stream.
        }
    }

    private fun sendFormat(config: ByteArray?) {
        if (config == null) return events.failed()
        formatSent = true
        events.format(ByteString.copyFrom(config), width, height)
    }

    private inner class Callback : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) = Unit

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
            if (released || formatSent) return
            val parts = listOf("csd-0", "csd-1").mapNotNull { format.getByteBuffer(it)?.toArray() }
            // Without parameter sets here, the encoder tells them in its first buffer.
            if (parts.isNotEmpty()) sendFormat(AvcStream.decoderConfig(*parts.toTypedArray()))
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            if (released) return
            try {
                val buffer = codec.getOutputBuffer(index)
                if (buffer != null && info.size > 0) {
                    buffer.position(info.offset)
                    buffer.limit(info.offset + info.size)
                    val bytes = buffer.toArray()
                    when {
                        info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0 -> if (!formatSent) sendFormat(AvcStream.decoderConfig(bytes))
                        else -> {
                            // An encoder that told its parameter sets nowhere else has them in its first key frame.
                            if (!formatSent) sendFormat(AvcStream.decoderConfig(bytes))
                            if (formatSent) {
                                events.frame(info.presentationTimeUs, info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0, AvcStream.lengthPrefixed(bytes))
                            }
                        }
                    }
                }
                codec.releaseOutputBuffer(index, false)
            } catch (_: IllegalStateException) {
                // Released or failed meanwhile.
            }
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            Log.w(TAG, "The screen's encoder failed: ${e.diagnosticInfo}")
            if (!released) events.failed()
        }
    }

    companion object {
        private const val TAG = "link2linux"
        private const val MIME = MediaFormat.MIMETYPE_VIDEO_AVC

        /** The longest side when the desktop leaves it to the phone. */
        private const val DEFAULT_MAX_SIZE = 1920
        private const val FRAME_RATE = 60
        private const val KEY_FRAME_SECONDS = 5

        /** While the screen does not change, the last frame again this often. */
        private const val REPEAT_AFTER_US = 100_000L
        private const val BITS_PER_PIXEL = 4L
        private const val MIN_BITRATE = 500_000
        private const val MAX_BITRATE = 12_000_000L

        /** An H.264 encoder: one in hardware where there is one. */
        fun find(): MediaCodecInfo? {
            val encoders = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
                .filter { info -> info.isEncoder && info.supportedTypes.any { it.equals(MIME, ignoreCase = true) } }
            return encoders.firstOrNull(::isHardware) ?: encoders.firstOrNull()
        }

        /** An encoder for [screen] scaled to [maxSize], or null when the phone has none that takes it. */
        fun create(screen: ScreenSize, maxSize: Int, handler: Handler, events: EncoderEvents): AvcEncoder? {
            val info = find() ?: return null
            return try {
                val video = info.getCapabilitiesForType(MIME).videoCapabilities ?: return null
                val size = FrameSize.fit(
                    screen.width,
                    screen.height,
                    maxSize,
                    DEFAULT_MAX_SIZE,
                    video.widthAlignment,
                    video.heightAlignment,
                ) { width, height -> video.isSizeSupported(width, height) } ?: return null
                val bitrate = video.bitrateRange.clamp((size.width * size.height * BITS_PER_PIXEL).coerceIn(MIN_BITRATE.toLong(), MAX_BITRATE).toInt())
                AvcEncoder(info, size, bitrate, handler, events)
            } catch (e: IOException) {
                Log.w(TAG, "No H.264 encoder: ${e.javaClass.simpleName}")
                null
            } catch (e: RuntimeException) {
                Log.w(TAG, "No H.264 encoder for the screen: ${e.javaClass.simpleName}")
                null
            }
        }

        private fun formatFor(info: MediaCodecInfo, size: FrameSize, bitrate: Int): MediaFormat =
            MediaFormat.createVideoFormat(MIME, size.width, size.height).apply {
                setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
                setInteger(MediaFormat.KEY_FRAME_RATE, FRAME_RATE)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, KEY_FRAME_SECONDS)
                setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, REPEAT_AFTER_US)
                // Real time.
                setInteger(MediaFormat.KEY_PRIORITY, 0)
                baseline(info)?.let {
                    setInteger(MediaFormat.KEY_PROFILE, it.profile)
                    setInteger(MediaFormat.KEY_LEVEL, it.level)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    setFloat(MediaFormat.KEY_MAX_FPS_TO_ENCODER, FRAME_RATE.toFloat())
                    setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            }

        /** The constrained baseline profile at the highest level the encoder offers, else plain baseline. */
        private fun baseline(info: MediaCodecInfo): MediaCodecInfo.CodecProfileLevel? {
            val levels = info.getCapabilitiesForType(MIME).profileLevels
            val constrained = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
                levels.filter { it.profile == MediaCodecInfo.CodecProfileLevel.AVCProfileConstrainedBaseline }
            } else {
                emptyList()
            }
            return constrained.ifEmpty { levels.filter { it.profile == MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline } }
                .maxByOrNull { it.level }
        }

        private fun isHardware(info: MediaCodecInfo): Boolean = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            info.isHardwareAccelerated
        } else {
            !info.name.startsWith("OMX.google.") && !info.name.startsWith("c2.android.")
        }

        private fun ByteBuffer.toArray(): ByteArray {
            val copy = duplicate()
            return ByteArray(copy.remaining()).also { copy.get(it) }
        }
    }
}
