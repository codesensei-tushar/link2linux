package dev.link2linux.android.core.screen

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ScreenCodec
import dev.link2linux.protocol.v1.ScreenEndReason
import dev.link2linux.protocol.v1.ScreenFormat
import dev.link2linux.protocol.v1.ScreenInput
import dev.link2linux.protocol.v1.ScreenStart
import dev.link2linux.protocol.v1.screenFormat
import dev.link2linux.protocol.v1.screenFrame
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.IOException
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.max

/** What the phone does with its screen, for the screens that show it. */
sealed interface ShareState {
    data object Idle : ShareState

    /** It asks the user whether to share the screen with [desktopName]. */
    data class Asking(val desktopName: String) : ShareState

    /** It shares the screen with [desktopName]. */
    data class Sharing(val desktopName: String) : ShareState
}

/**
 * The phone's side of the screen feature (docs/protocol.md, "Screen"): a
 * desktop shows the phone's screen in a window and, where the user allows
 * it, controls the phone from there.
 *
 * One desktop at a time: a `ScreenStart` asks the user ([ScreenAsker]) and
 * is answered once the question is out; another desktop gets `BUSY` until
 * the stream ends. The user's consent hands the engine a [ScreenCapture],
 * whose frames go to the desktop through an [Outbox] that keeps the wait
 * short. A stream ends when the desktop stops it, the user stops it on the
 * phone, the capture stops or fails, the feature leaves `READY` on either
 * side, or the session ends; all but the first and the last tell the
 * desktop with `ScreenEnded`.
 *
 * Input is carried out only for the desktop the screen goes to, while it
 * goes there and while [control] is on, one input after the other in the
 * order they arrived.
 */
class ScreenEngine(
    private val scope: CoroutineScope,
    private val asker: ScreenAsker,
    /** The codecs this phone can encode. */
    private val encodable: Set<ScreenCodec>,
    private val controller: ScreenController,
    /** Whether the user has the feature on. */
    val enabled: StateFlow<Boolean>,
    /** Whether the desktop may control the phone: the user allows it and the accessibility service runs. */
    val control: StateFlow<Boolean>,
    /** Whether the phone is locked, asked when the capture stops by itself. */
    private val isLocked: () -> Boolean,
    private val answerMillis: Long = ANSWER_MILLIS,
) {

    private sealed interface State {
        val peer: ScreenPeer
    }

    private class Asking(override val peer: ScreenPeer, val askId: Long, var config: CaptureConfig) : State {
        var timeout: Job? = null
    }

    private inner class Sharing(val stream: Stream) : State {
        override val peer: ScreenPeer get() = stream.peer
    }

    private val lock = Mutex()

    // Written under lock; read without it to turn input away early.
    @Volatile
    private var state: State? = null
    private var lastAskId = 0L // guarded by lock
    private val formatIds = AtomicInteger()

    private val _share = MutableStateFlow<ShareState>(ShareState.Idle)

    /** What the phone does with its screen now. */
    val share: StateFlow<ShareState> = _share.asStateFlow()

    init {
        scope.launch {
            enabled.collect { on -> if (!on) endAny(ScreenEndReason.SCREEN_END_REASON_INTERRUPTED) }
        }
        scope.launch {
            control.collect { on -> (state as? Sharing)?.stream?.setInput(on) }
        }
    }

    /**
     * A `ScreenStart` from [peer]. [answer] gets the answer before anything
     * else of this stream goes out, so a session that queues it there sends
     * them in order.
     */
    suspend fun start(peer: ScreenPeer, request: ScreenStart, answer: (ScreenReply) -> Unit) = lock.withLock {
        if (!enabled.value) return@withLock answer(refused(ErrorCode.ERROR_CODE_DISABLED, "Screen sharing is turned off on the phone"))
        val config = configFor(request).getOrElse { return@withLock answer(refused(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, it.message.orEmpty())) }
        state?.let { current ->
            if (current.peer !== peer) {
                if (current.peer.desktopId != peer.desktopId) {
                    return@withLock answer(refused(ErrorCode.ERROR_CODE_BUSY, "The phone shares its screen with another desktop"))
                }
                // The same desktop's older session, about to end: this one takes its place.
                finish(current, null)
            }
        }
        when (val current = state) {
            is Asking -> {
                current.config = config
                return@withLock answer(ScreenReply.Done)
            }
            is Sharing -> {
                answer(ScreenReply.Done)
                // No new question: the user agreed to share with this desktop already.
                return@withLock current.stream.restart(config)
            }
            null -> Unit
        }
        val askId = ++lastAskId
        if (!asker.ask(askId, peer.desktopName)) {
            return@withLock answer(refused(ErrorCode.ERROR_CODE_NOT_ALLOWED, "The phone cannot ask its user"))
        }
        answer(ScreenReply.Done)
        val asking = Asking(peer, askId, config)
        asking.timeout = scope.launch {
            delay(answerMillis)
            lock.withLock { finish(asking, ScreenEndReason.SCREEN_END_REASON_TIMED_OUT) }
        }
        state = asking
        _share.value = ShareState.Asking(peer.desktopName)
    }

    /** A `ScreenStop` from [peer]: stops its stream, or the question for it, without `ScreenEnded`. Then [answer] gets `Ack`. */
    suspend fun stop(peer: ScreenPeer, answer: (ScreenReply) -> Unit) = lock.withLock {
        state?.takeIf { it.peer === peer }?.let { finish(it, null) }
        answer(ScreenReply.Done)
    }

    /** [peer]'s session ended, and with it its stream, without a message. */
    suspend fun sessionEnded(peer: ScreenPeer) = lock.withLock {
        state?.takeIf { it.peer === peer }?.let { finish(it, null) }
    }

    /** [peer]'s `FeatureStatus` says the feature is not `READY` there. */
    suspend fun notReadyOn(peer: ScreenPeer) = lock.withLock {
        state?.takeIf { it.peer === peer }?.let { finish(it, ScreenEndReason.SCREEN_END_REASON_INTERRUPTED) }
    }

    /** A `ScreenKeyFrameRequest` from [peer]. */
    fun keyFrameRequested(peer: ScreenPeer) {
        (state as? Sharing)?.stream?.takeIf { it.peer === peer }?.keyFrameRequested()
    }

    /** A `ScreenInput` from [peer]; [answer] gets the answer once it is carried out, or turned away. */
    fun input(peer: ScreenPeer, input: ScreenInput, answer: (ScreenReply) -> Unit) {
        val stream = (state as? Sharing)?.stream?.takeIf { it.peer === peer }
        if (stream == null || !control.value) return answer(NOT_ACCEPTED)
        stream.input(input, answer)
    }

    /** Whether the question [askId] still waits for the user's answer. */
    fun isAsking(askId: Long): Boolean = (state as? Asking)?.askId == askId

    /**
     * The user agreed to the question [askId], and [capture] is ready. A
     * capture that comes too late, after the question was taken back, is
     * stopped.
     */
    fun granted(askId: Long, capture: ScreenCapture) {
        scope.launch {
            lock.withLock {
                val asking = state as? Asking
                if (asking?.askId != askId) return@withLock capture.stop()
                asking.timeout?.cancel()
                asker.withdraw(askId)
                val stream = Stream(asking.peer, capture)
                val sharing = Sharing(stream)
                state = sharing
                _share.value = ShareState.Sharing(asking.peer.desktopName)
                stream.open { reason -> scope.launch { lock.withLock { finish(sharing, reason) } } }
                if (!capture.start(asking.config, stream)) finish(sharing, ScreenEndReason.SCREEN_END_REASON_INTERRUPTED)
            }
        }
    }

    /** The user declined the question [askId]. */
    fun declined(askId: Long) = endAsking(askId, ScreenEndReason.SCREEN_END_REASON_DECLINED)

    /** The user agreed to the question [askId], but the phone could not start capturing. */
    fun couldNotStart(askId: Long) = endAsking(askId, ScreenEndReason.SCREEN_END_REASON_INTERRUPTED)

    /** The user stopped sharing on the phone. */
    fun stopOnPhone() {
        scope.launch { lock.withLock { (state as? Sharing)?.let { finish(it, ScreenEndReason.SCREEN_END_REASON_STOPPED_ON_PHONE) } } }
    }

    private fun endAsking(askId: Long, reason: ScreenEndReason) {
        scope.launch { lock.withLock { (state as? Asking)?.takeIf { it.askId == askId }?.let { finish(it, reason) } } }
    }

    private suspend fun endAny(reason: ScreenEndReason) = lock.withLock { state?.let { finish(it, reason) } }

    /**
     * Ends [ended] unless something else did already, and tells its desktop
     * [reason], or nothing when null. Under lock. From the question's own
     * timeout, nothing here suspends after that timeout is cancelled.
     */
    private suspend fun finish(ended: State, reason: ScreenEndReason?) {
        if (state !== ended) return
        state = null
        _share.value = ShareState.Idle
        when (ended) {
            is Asking -> {
                ended.timeout?.cancel()
                asker.withdraw(ended.askId)
            }
            is Sharing -> ended.stream.close()
        }
        if (reason != null) {
            scope.launch {
                try {
                    ended.peer.ended(reason)
                } catch (_: IOException) {
                    // The connection failed; the session ends with it.
                }
            }
        }
    }

    private fun configFor(request: ScreenStart): Result<CaptureConfig> {
        val codecs = request.codecsList
        val maxSize = Integer.toUnsignedLong(request.maxSize)
        return when {
            codecs.size !in 1..MAX_CODECS -> Result.failure(IllegalArgumentException("${codecs.size} codecs, outside 1 to $MAX_CODECS"))
            maxSize != 0L && maxSize !in MIN_SIZE..MAX_SIZE ->
                Result.failure(IllegalArgumentException("A longest side of $maxSize, outside $MIN_SIZE to $MAX_SIZE"))
            else -> codecs.firstOrNull { it in encodable }
                ?.let { Result.success(CaptureConfig(it, maxSize.toInt())) }
                ?: Result.failure(IllegalArgumentException("None of the codecs can be encoded here"))
        }
    }

    /** An input waiting for its turn. */
    private class PendingInput(val input: ScreenInput, val answer: (ScreenReply) -> Unit)

    /**
     * One stream to [peer], from the user's consent until it ends. The
     * capture reports here on its own threads, so what it touches is
     * guarded by the stream itself; the sender and the input worker are
     * coroutines of their own.
     */
    private inner class Stream(val peer: ScreenPeer, private val capture: ScreenCapture) : CaptureSink {
        private val outbox = Outbox()
        private val carrier = InputCarrier(controller)
        private val inputs = Channel<PendingInput>(MAX_WAITING_INPUTS)
        private val rate = FrameRate()
        private val quality = Quality()
        private lateinit var onEnd: (ScreenEndReason) -> Unit
        private var sender: Job? = null
        private var worker: Job? = null

        // Guarded by this.
        private var ended = false
        private var format: ScreenFormat? = null
        private var input = control.value
        private var lastTimestampUs = -1L
        private var keyFrameAskedAtUs: Long? = null

        /** The video the desktop sees now, as the input worker maps its points. */
        @Volatile
        private var video: VideoFormat? = null

        /** Starts sending and carrying out input. [onEnd] ends the stream, from any thread. */
        fun open(onEnd: (ScreenEndReason) -> Unit) {
            this.onEnd = onEnd
            sender = scope.launch {
                try {
                    while (true) {
                        when (val item = outbox.next() ?: break) {
                            is Outbox.Item.Format -> peer.format(item.format)
                            is Outbox.Item.Frame -> peer.frame(item.frame)
                        }
                        outbox.written()
                    }
                } catch (_: IOException) {
                    // The connection failed; the session ends with it, and so does the stream.
                }
            }
            worker = scope.launch {
                for (pending in inputs) {
                    val reply = try {
                        if (control.value) carrier.carryOut(pending.input, video) else NOT_ACCEPTED
                    } catch (e: CancellationException) {
                        pending.answer(NOT_ACCEPTED)
                        throw e
                    }
                    pending.answer(reply)
                }
            }
        }

        /**
         * Ends the stream: the capture stops, and once this returns nothing
         * more of it goes to the session, so an answer queued after this goes
         * out after its last frame. Input still waiting is turned away.
         */
        suspend fun close() {
            synchronized(this) { ended = true }
            capture.stop()
            outbox.close()
            sender?.cancelAndJoin()
            worker?.cancelAndJoin()
            inputs.close()
            while (true) {
                val left = inputs.tryReceive().getOrNull() ?: break
                left.answer(NOT_ACCEPTED)
            }
            // The worker has finished, so nothing will read the password it typed again.
            carrier.forget()
        }

        /** The desktop asked again: encode as it now asks, under a new format id, starting with a key frame. */
        fun restart(config: CaptureConfig) = capture.reconfigure(config)

        fun keyFrameRequested() {
            // The desktop drops what comes before the key frame anyway.
            outbox.skipToKeyFrame()
            synchronized(this) { keyFrameAskedAtUs = lastTimestampUs }
            capture.requestKeyFrame()
        }

        fun input(input: ScreenInput, answer: (ScreenReply) -> Unit) {
            val sent = inputs.trySend(PendingInput(input, answer))
            when {
                sent.isSuccess -> Unit
                sent.isClosed -> answer(NOT_ACCEPTED)
                else -> answer(refused(ErrorCode.ERROR_CODE_BUSY, "More than $MAX_WAITING_INPUTS inputs wait"))
            }
        }

        /** Control turned on or off: the desktop learns it from a format with the same id. */
        fun setInput(on: Boolean) {
            synchronized(this) {
                if (ended || input == on) return
                input = on
                val current = format ?: return
                format = current.toBuilder().setInput(on).build().also { outbox.format(it, newVideo = false) }
            }
        }

        override fun onFormat(format: CaptureFormat) {
            val valid = format.width > 0 && format.height > 0 && format.screenWidth > 0 && format.screenHeight > 0 &&
                format.codecConfig.size() <= AvcStream.MAX_CONFIG_BYTES
            synchronized(this) {
                if (ended) return
                if (valid) {
                    val id = formatIds.incrementAndGet()
                    video = VideoFormat(id, format.width, format.height, format.screenWidth, format.screenHeight)
                    keyFrameAskedAtUs = null
                    this.format = format.toProto(id, input).also { outbox.format(it, newVideo = true) }
                    return
                }
            }
            onEnd(ScreenEndReason.SCREEN_END_REASON_INTERRUPTED)
        }

        override fun onFrame(timestampUs: Long, keyFrame: Boolean, data: ByteArray): Unit = synchronized(this) {
            if (ended || format == null) return
            // Timestamps grow from frame to frame, also across an encoder restart.
            val at = max(timestampUs, lastTimestampUs + 1)
            if (data.size > MAX_FRAME_BYTES) {
                outbox.skipToKeyFrame()
                quality.lower(at)?.let(capture::setQuality)
                requestKeyFrameSoon(at)
                return
            }
            if (!rate.take(at)) {
                outbox.skipToKeyFrame()
                requestKeyFrameSoon(at)
                return
            }
            val frame = screenFrame {
                this.timestampUs = at
                this.keyFrame = keyFrame
                this.data = ByteString.copyFrom(data)
            }
            when (outbox.frame(frame)) {
                Outbox.Offer.QUEUED -> {
                    lastTimestampUs = at
                    if (keyFrame) keyFrameAskedAtUs = null
                    quality.fine(at)?.let(capture::setQuality)
                }
                Outbox.Offer.DROPPED -> {
                    quality.lower(at)?.let(capture::setQuality)
                    requestKeyFrameSoon(at)
                }
                // A request the capture missed is repeated, so that a stall never lasts.
                Outbox.Offer.WAITING -> requestKeyFrameSoon(at)
                Outbox.Offer.CLOSED -> Unit
            }
        }

        override fun onStopped() {
            onEnd(if (isLocked()) ScreenEndReason.SCREEN_END_REASON_LOCKED else ScreenEndReason.SCREEN_END_REASON_STOPPED_ON_PHONE)
        }

        override fun onFailed() = onEnd(ScreenEndReason.SCREEN_END_REASON_INTERRUPTED)

        /**
         * Asks the capture for a key frame, at most once a second. Every
         * dropped or oversized frame wants one, and a capture that answers by
         * sending the frame again (a still picture) would otherwise be asked
         * again for that frame, without end.
         *
         * Called holding this.
         */
        private fun requestKeyFrameSoon(atUs: Long) {
            val askedAt = keyFrameAskedAtUs
            if (askedAt != null && atUs - askedAt < KEY_FRAME_RETRY_US) return
            askForKeyFrame(atUs)
        }

        // Called holding this.
        private fun askForKeyFrame(atUs: Long) {
            keyFrameAskedAtUs = atUs
            capture.requestKeyFrame()
        }
    }

    private fun CaptureFormat.toProto(id: Int, input: Boolean): ScreenFormat = screenFormat {
        this.id = id
        codec = this@toProto.codec
        width = this@toProto.width
        height = this@toProto.height
        codecConfig = this@toProto.codecConfig
        this.input = input
    }

    companion object {
        /** How long the user has to answer (docs/protocol.md, "Starting and stopping"). */
        const val ANSWER_MILLIS: Long = 60_000

        /** docs/protocol.md, "Limits". */
        const val MAX_CODECS = 8
        const val MIN_SIZE = 240L
        const val MAX_SIZE = 4096L
        const val MAX_FRAME_BYTES = 448 * 1024

        /** Inputs that may wait behind the one being carried out; more are answered `BUSY`. */
        const val MAX_WAITING_INPUTS = 32

        /** How long the frames wait for a key frame before the engine asks the capture again. */
        const val KEY_FRAME_RETRY_US = 1_000_000L

        private val NOT_ACCEPTED = ScreenReply.Refused(ErrorCode.ERROR_CODE_NOT_ALLOWED, "The phone does not take input from this desktop now")

        private fun refused(code: ErrorCode, detail: String) = ScreenReply.Refused(code, detail)
    }
}
