package dev.link2linux.android.core.screen

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ScreenCodec
import dev.link2linux.protocol.v1.ScreenEndReason
import dev.link2linux.protocol.v1.ScreenFormat
import dev.link2linux.protocol.v1.ScreenFrame
import dev.link2linux.protocol.v1.ScreenKey

/*
 * The seams between the screen engine, the phone and the session. The
 * platform asks the user, captures and encodes the screen, and carries out
 * the desktop's input; the session attaches a [ScreenPeer] for each desktop
 * where the feature is active and passes the desktop's messages to the
 * engine.
 */

/** A connected desktop where the feature is active. */
interface ScreenPeer {
    /** Names the desktop, the same in each of its sessions. */
    val desktopId: String

    /** What the user calls the desktop, for the question on the phone. */
    val desktopName: String

    /** Sends `ScreenFormat` and returns once it is written. Throws when the connection fails. */
    suspend fun format(format: ScreenFormat)

    /** Sends `ScreenFrame` and returns once it is written. Throws when the connection fails. */
    suspend fun frame(frame: ScreenFrame)

    /** Sends `ScreenEnded` with [reason]. Throws when the connection fails. */
    suspend fun ended(reason: ScreenEndReason)
}

/** The phone's answer to a desktop's request. */
sealed interface ScreenReply {
    /** `Ack`. */
    data object Done : ScreenReply

    /** An `ErrorReply`; [detail] is English for logs and never names the user's content. */
    data class Refused(val code: ErrorCode, val detail: String) : ScreenReply
}

/** Asks the user whether to share the screen, once per stream. */
interface ScreenAsker {
    /**
     * Asks whether to share the screen with [desktopName], or returns false
     * when it cannot ask, for example because the user blocked the app's
     * notifications while the app is not in front. The user's answer comes
     * back through [ScreenEngine.granted] or [ScreenEngine.declined], with
     * [askId].
     */
    fun ask(askId: Long, desktopName: String): Boolean

    /** Takes the question [askId] back: it was answered, timed out or is no longer wanted. */
    fun withdraw(askId: Long)
}

/** What the engine wants the capture to send: the codec, and the longest side, or 0 to leave it to the phone. */
data class CaptureConfig(val codec: ScreenCodec, val maxSize: Int)

/**
 * How the capture encodes the frames that follow: the codec, the frames'
 * size, the codec's configuration (the avcC record for H.264, empty for
 * JPEG), and the size of the screen the frames show, in its current
 * orientation.
 */
data class CaptureFormat(
    val codec: ScreenCodec,
    val width: Int,
    val height: Int,
    val codecConfig: ByteString,
    val screenWidth: Int,
    val screenHeight: Int,
)

/**
 * One screen capture that the user agreed to: on Android, a media
 * projection. Each one serves a single stream; [stop] gives it back.
 */
interface ScreenCapture {
    /**
     * Starts capturing and encoding as [config] says, telling [sink] what
     * it encodes. Returns false when it cannot. Called once.
     */
    fun start(config: CaptureConfig, sink: CaptureSink): Boolean

    /**
     * Encodes as [config] says from now on, also when it says the same as
     * before: the sink then gets [CaptureSink.onFormat], and a key frame.
     */
    fun reconfigure(config: CaptureConfig)

    /** Makes the next frame a key frame, within a second, also while the screen does not change. */
    fun requestKeyFrame()

    /** Scales the bitrate, or the JPEG quality, down to [quality] of the full one: above 0, at most 1. */
    fun setQuality(quality: Float)

    /** Stops capturing and gives the capture back. Whatever it tells the sink after this is ignored. */
    fun stop()
}

/** Where a [ScreenCapture] reports, on any thread. Each call returns quickly. */
interface CaptureSink {
    /** A new encoding starts: at the start, and after each change of size or configuration. A key frame follows. */
    fun onFormat(format: CaptureFormat)

    /**
     * One encoded frame: for H.264 one access unit with 4-byte lengths in
     * front of its NAL units, for JPEG a whole image. [timestampUs] is when
     * it was captured.
     */
    fun onFrame(timestampUs: Long, keyFrame: Boolean, data: ByteArray)

    /** The capture stopped by itself: the user stopped it, or the phone locked. */
    fun onStopped()

    /** The capture cannot go on, for example because its encoder failed. */
    fun onFailed()
}

/** A point on the phone's screen, in pixels of its current orientation. */
data class ScreenPointF(val x: Float, val y: Float)

/**
 * A part of one finger's touch: it follows [points] in [durationMillis].
 * One point stays at that point; consecutive strokes continue each other.
 */
data class Stroke(val points: List<ScreenPointF>, val durationMillis: Long)

/** The text field that has the input focus, as it is when it was looked up. */
interface TextField {
    /** Names the field: equal for the same field each time it is looked up, and different for another. */
    val identity: Any

    /** Its content; empty while it shows its hint. For a password, Android gives as many dots as it holds. */
    val text: String

    /** The selection, as indices into [text]: equal for a cursor, -1 when the field does not say. */
    val selectionStart: Int
    val selectionEnd: Int

    val isPassword: Boolean
    val isMultiLine: Boolean

    /** Whether the field offers the keyboard's action key, such as send or search. */
    val canEnter: Boolean

    /** Replaces the content with [text] and puts the cursor at [cursor]. Returns whether that worked. */
    fun setText(text: String, cursor: Int): Boolean

    /** Presses the keyboard's action key. Returns whether that worked. */
    fun enter(): Boolean
}

/** Carries out the desktop's input on the phone: on Android, the accessibility service. */
interface ScreenController {
    /** Touches the screen along [strokes], one after the other. Returns whether Android carried the touch out. */
    suspend fun gesture(strokes: List<Stroke>): Boolean

    /** One of the system's navigation keys: back, home, recents or notifications. Returns whether it worked. */
    fun navigate(key: ScreenKey): Boolean

    /** The text field that has the input focus, or null when none has. */
    fun focusedField(): TextField?
}
