package dev.link2linux.android.core.network

import com.google.protobuf.InvalidProtocolBufferException
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.Goodbye
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.Pong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.Closeable
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * One connection's worth of envelopes over an established [Transport].
 *
 * Both sides send `Hello` as their first frame (seq 1), then read the
 * peer's, and use the highest protocol version both ranges contain. Each
 * peer's sequence numbers must go up by exactly one. [next] and [incoming]
 * answer `Ping` with `Pong` themselves, hand the answers to this side's own
 * requests ([sendRequest]) to whoever waits for them, and keep what the
 * peer's latest `FeatureStatus` says ([peerFeatures]). A protocol error
 * sends `Goodbye` with [GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR] and
 * closes.
 *
 * Every frame goes out through one writer, in the order it was queued:
 * [send] waits until its frame is written, [post] does not. The reader
 * never waits for a write. It posts its `Pong`s, and features post the
 * answers they give while reading: two sides that both stopped reading
 * while writing bulk data would wait for each other once the network's
 * buffers are full (docs/protocol.md, "Framing").
 *
 * Liveness is the transport's: on the phone, a read that waits longer than
 * the idle timeout fails, and that ends the session. Writes have their own
 * deadline, [writeTimeoutMillis]: a peer that stops reading would otherwise
 * hold up every frame of this session. A write that fails ends the session,
 * since a frame cut short leaves nothing readable after it.
 *
 * Until [markPaired] says the desktop is paired, frames are limited to
 * [FrameCodec.HANDSHAKE_MAX_FRAME_BYTES]: the Hello exchange and the
 * pairing messages need far less, and the peer may be anyone.
 */
class Session(
    private val transport: Transport,
    private val codec: FrameCodec = FrameCodec(),
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    /** How long writing one envelope may take before the connection is closed. */
    private val writeTimeoutMillis: Long = WRITE_TIMEOUT_MILLIS,
) : Closeable {

    @Volatile
    private var frames = FrameCodec(minOf(codec.maxFrameBytes, FrameCodec.HANDSHAKE_MAX_FRAME_BYTES))

    private var nextSeq = 1L // touched only by the writer
    private var lastPeerSeq = 0L // touched only by the single reader

    /** One frame for the writer. */
    private class Write(
        val fill: Envelope.Builder.() -> Unit,
        /** A request's answer: it waits from just before the frame goes out, so the answer cannot come first. */
        val answer: CompletableDeferred<Envelope>? = null,
        /** Gets the frame's seq once it is written; null for a posted frame, which nobody waits for. */
        val written: CompletableDeferred<Long>? = null,
        /** A posted frame's size, counted against [MAX_POSTED_BYTES] until the writer takes it. */
        val postedBytes: Int = 0,
    )

    /** The frames waiting for the writer, in order. Closing the session fails them. */
    private val writes = Channel<Write>(Channel.UNLIMITED) { it.written?.completeExceptionally(SessionException("The session ended")) }

    // Posted frames the writer has not taken yet.
    private val postedFrames = AtomicInteger()
    private val postedBytes = AtomicLong()

    /** This side's requests that wait for their answer, by seq. */
    private val waiting = ConcurrentHashMap<Long, CompletableDeferred<Envelope>>()

    @Volatile
    private var localFeatures: List<String> = emptyList()

    private val _peerFeatures = MutableStateFlow<Map<String, FeatureAvailability>>(emptyMap())

    /**
     * What the peer's latest `FeatureStatus` says, by feature name: each one
     * replaces the last. Empty until the first arrives; a feature it does not
     * list is not ready there. Updated before [next] returns the status.
     */
    val peerFeatures: StateFlow<Map<String, FeatureAvailability>> = _peerFeatures.asStateFlow()

    private val _state = MutableStateFlow<SessionState>(SessionState.Handshaking)
    val state: StateFlow<SessionState> = _state.asStateFlow()

    init {
        // Last, once everything the writer touches exists. It ends when the session closes.
        CoroutineScope(ioDispatcher).launch { writeAll() }
    }

    /**
     * Exchanges `Hello`s and returns the peer's. Closes the session on
     * failure, and when the exchange takes longer than [timeoutMillis]; when
     * the version ranges do not overlap it first sends `Goodbye` with
     * [GoodbyeReason.GOODBYE_REASON_VERSION_MISMATCH].
     */
    suspend fun handshake(local: Hello, timeoutMillis: Long = HANDSHAKE_TIMEOUT_MILLIS): Hello = try {
        closingAfter(timeoutMillis, "The Hello exchange") {
            localFeatures = local.featuresList
            send { hello = local }
            val first = receive() ?: throw SessionException("Peer closed the stream before its Hello")
            if (!first.hasHello()) protocolError("Expected Hello first, got ${first.payloadCase}")
            val peer = first.hello
            val version = negotiateVersion(local, peer)
            if (version == null) {
                goodbye(GoodbyeReason.GOODBYE_REASON_VERSION_MISMATCH, "No common protocol version")
                throw VersionMismatchException(local, peer)
            }
            _state.value = SessionState.Established(peer, version)
            peer
        }
    } catch (e: Exception) {
        close()
        throw e
    }

    /**
     * Starts the session proper once the desktop is known to be paired:
     * after the Hello exchange on a reconnect, or once a new pairing is
     * stored. Lifts the frame limit of the handshake.
     */
    fun markPaired() {
        frames = codec
    }

    /** Sends one envelope and returns its sequence number once it is written. */
    suspend fun send(fill: Envelope.Builder.() -> Unit): Long = write(fill)

    /**
     * Queues one envelope and returns at once, without waiting for it to be
     * written: for answers given while reading, since the reader must never
     * wait for a write. [fill] runs at once, so the queue holds the answer
     * alone and not what it was made from, such as the request. The envelope
     * goes out in order behind those queued before it, within the same
     * deadline, and a failed write ends the session. A peer that leaves more
     * than [MAX_POSTED] posted frames, or [MAX_POSTED_BYTES], waiting has
     * stopped reading, and is cut off.
     */
    fun post(fill: Envelope.Builder.() -> Unit) {
        val answer = Envelope.newBuilder().apply(fill).build()
        val size = answer.serializedSize
        if (postedFrames.incrementAndGet() > MAX_POSTED || postedBytes.addAndGet(size.toLong()) > MAX_POSTED_BYTES) {
            close()
            return
        }
        writes.trySend(Write(fill = { mergeFrom(answer) }, postedBytes = size))
    }

    /**
     * Sends a request (`expects_reply` set) and returns once it is written,
     * so that a caller can keep several open, sent in the order it calls.
     * [PendingRequest.await] gives the peer's answer: the envelope whose
     * `reply_to` is the request's seq. [timeoutMillis] count from the
     * moment the request is written. Only for requests the peer answers
     * with one envelope.
     */
    suspend fun sendRequest(timeoutMillis: Long = REQUEST_TIMEOUT_MILLIS, fill: Envelope.Builder.() -> Unit): PendingRequest {
        val answer = CompletableDeferred<Envelope>()
        val seq = try {
            write({ fill(); expectsReply = true }, answer)
        } catch (e: Throwable) {
            // By value: a seq may since belong to another request.
            waiting.values.remove(answer)
            throw e
        }
        // Closed meanwhile: close() may have missed this request.
        if (_state.value == SessionState.Closed) answer.completeExceptionally(SessionException("The session ended"))
        return PendingRequest(seq, answer, System.nanoTime() + timeoutMillis * NANOS_PER_MILLI) { waiting.remove(seq, answer) }
    }

    /** Sends a request and waits for its answer: [sendRequest], then [PendingRequest.await]. */
    suspend fun request(timeoutMillis: Long = REQUEST_TIMEOUT_MILLIS, fill: Envelope.Builder.() -> Unit): Envelope =
        sendRequest(timeoutMillis, fill).await()

    /** Whether [feature], a wire name, is active in this session: both Hellos list it. */
    fun isActive(feature: String): Boolean {
        val state = _state.value
        return state is SessionState.Established && feature in state.peer.featuresList && feature in localFeatures
    }

    /** Whether the peer's latest `FeatureStatus` says [feature] is `READY` there. */
    fun isReadyOnPeer(feature: String): Boolean =
        peerFeatures.value[feature] == FeatureAvailability.FEATURE_AVAILABILITY_READY

    private suspend fun write(fill: Envelope.Builder.() -> Unit, answer: CompletableDeferred<Envelope>? = null): Long {
        val written = CompletableDeferred<Long>()
        if (writes.trySend(Write(fill, answer, written)).isFailure) throw SessionException("The session ended")
        return try {
            written.await()
        } catch (e: CancellationException) {
            // A frame whose turn has not come yet is not written at all.
            written.cancel()
            throw e
        }
    }

    /**
     * The writer: writes the queued frames one at a time, each within
     * [writeTimeoutMillis]. A frame that cannot be built, or is too large,
     * fails before it takes a seq or registers as a request.
     */
    private suspend fun writeAll() {
        for (write in writes) {
            if (write.written == null) {
                postedFrames.decrementAndGet()
                postedBytes.addAndGet(-write.postedBytes.toLong())
            }
            if (write.written?.isCancelled == true) continue
            val seq = nextSeq
            val envelope = try {
                Envelope.newBuilder().apply(write.fill).setSeq(seq).build().also(frames::checkSize)
            } catch (e: Exception) {
                write.written?.completeExceptionally(e)
                continue
            }
            nextSeq++
            try {
                write.answer?.let { waiting[seq] = it }
                closingAfter(writeTimeoutMillis, "Writing envelope $seq") {
                    frames.write(envelope, transport.output)
                    transport.output.flush()
                }
                // Its sender gave up while it went out, so nobody will take the answer.
                if (write.written?.complete(seq) == false) write.answer?.let { waiting.remove(seq, it) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                write.written?.completeExceptionally(e)
                close()
                return
            }
        }
    }

    /**
     * The peer's next envelope after the Hello exchange, or null when it
     * closed the stream. Answers `Ping` without returning it; a second
     * `Hello` is a protocol error.
     */
    suspend fun next(): Envelope? {
        while (true) {
            val envelope = receive() ?: return null
            when {
                envelope.hasPing() -> post { setPong(Pong.newBuilder().setNonce(envelope.ping.nonce)) }
                envelope.hasHello() -> protocolError("Second Hello")
                envelope.replyTo != 0L && answered(envelope) -> Unit
                else -> {
                    if (envelope.hasFeatureStatus()) {
                        _peerFeatures.value = envelope.featureStatus.featuresList.associate { it.feature to it.availability }
                    }
                    return envelope
                }
            }
        }
    }

    /** Hands [envelope] to the request it answers; false when none waits for it. */
    private fun answered(envelope: Envelope): Boolean {
        val request = waiting.remove(envelope.replyTo) ?: return false
        request.complete(envelope)
        return true
    }

    /** The peer's envelopes, as from [next], until it closes the stream; then the session closes. */
    fun incoming(): Flow<Envelope> = flow {
        while (true) emit(next() ?: break)
        close()
    }

    /**
     * Sends `Goodbye` and closes the connection. The send is best effort: it
     * waits at most [GOODBYE_GRACE_MILLIS] behind the frames queued before it.
     */
    suspend fun goodbye(reason: GoodbyeReason, detail: String = "") {
        runCatching {
            withTimeoutOrNull(GOODBYE_GRACE_MILLIS) {
                send { goodbye = Goodbye.newBuilder().setReason(reason).setDetail(detail).build() }
            }
        }
        close()
    }

    /** Ends the session for a protocol error: `Goodbye`, close, then throws [SessionException]. */
    suspend fun protocolError(detail: String): Nothing {
        goodbye(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, detail)
        throw SessionException(detail)
    }

    override fun close() {
        _state.value = SessionState.Closed
        transport.close()
        // Nothing more goes out, and nothing answers this side's requests any more.
        writes.cancel()
        waiting.values.forEach { it.completeExceptionally(SessionException("The session ended")) }
    }

    /**
     * Runs [block], and closes the connection if it takes longer than
     * [millis]: that ends a read or write it is blocked in, which a
     * coroutine timeout cannot.
     */
    private suspend fun <T> closingAfter(millis: Long, what: String, block: suspend () -> T): T = coroutineScope {
        val expired = AtomicBoolean(false)
        val watchdog = launch(ioDispatcher) {
            delay(millis)
            expired.set(true)
            close()
        }
        try {
            block()
        } catch (e: IOException) {
            throw if (expired.get()) SessionException("$what took longer than $millis ms") else e
        } finally {
            watchdog.cancel()
        }
    }

    private suspend fun receive(): Envelope? {
        val envelope = try {
            withContext(ioDispatcher) { frames.read(transport.input) }
        } catch (e: FrameLimitException) {
            protocolError(e.message.orEmpty())
        } catch (e: InvalidProtocolBufferException) {
            protocolError(e.message.orEmpty())
        } ?: return null
        if (envelope.seq != lastPeerSeq + 1) protocolError("Peer sent seq ${envelope.seq} after $lastPeerSeq")
        lastPeerSeq = envelope.seq
        return envelope
    }

    companion object {
        /** How long the Hello exchange may take, as on the desktop. */
        const val HANDSHAKE_TIMEOUT_MILLIS: Long = 10_000

        /**
         * How long one envelope may take to write. The largest, an image of
         * 1 MiB, needs far less on any working local network.
         */
        const val WRITE_TIMEOUT_MILLIS: Long = 30_000

        /** How long a request waits for its answer (docs/protocol.md, "Requests and replies"). */
        const val REQUEST_TIMEOUT_MILLIS: Long = 30_000

        /** How long [goodbye] waits for its frame to go out before it closes anyway. */
        const val GOODBYE_GRACE_MILLIS: Long = 2_000

        /** Posted frames that may wait for the writer; more means the peer stopped reading. */
        const val MAX_POSTED: Int = 1024

        /** The bytes posted frames may hold while they wait. Answers are small; this is about a thousand of them. */
        const val MAX_POSTED_BYTES: Long = 1024 * 1024

        private const val NANOS_PER_MILLI = 1_000_000L

        /**
         * The highest protocol version both Hellos' ranges contain, or null
         * when they do not overlap. A `min_protocol_version` of 0 means the
         * same as `protocol_version`.
         */
        fun negotiateVersion(local: Hello, peer: Hello): Int? {
            fun Hello.min() = if (minProtocolVersion == 0) protocolVersion else minProtocolVersion
            val highest = minOf(local.protocolVersion, peer.protocolVersion)
            val lowest = maxOf(local.min(), peer.min())
            return highest.takeIf { it >= lowest && it > 0 }
        }
    }
}

sealed interface SessionState {
    data object Handshaking : SessionState
    data class Established(val peer: Hello, val version: Int) : SessionState
    data object Closed : SessionState
}

open class SessionException(message: String) : IOException(message)

/**
 * A request this side sent, waiting for the peer's answer. Await it once:
 * when [await] returns or fails, the request stops waiting, and a later
 * answer reaches [Session.next] like any other envelope.
 */
class PendingRequest internal constructor(
    /** The request's seq, which its answer names in `reply_to`. */
    val seq: Long,
    private val answer: CompletableDeferred<Envelope>,
    private val deadlineNanos: Long,
    private val forget: () -> Unit,
) {
    /**
     * The peer's answer. One that arrived in time is returned even when
     * this is called after the deadline.
     *
     * @throws RequestTimeoutException when none arrived before the deadline.
     * @throws SessionException when the session ended first.
     */
    suspend fun await(): Envelope = try {
        if (answer.isCompleted) {
            answer.await()
        } else {
            val leftMillis = (deadlineNanos - System.nanoTime() + 999_999) / 1_000_000
            withTimeoutOrNull(leftMillis) { answer.await() } ?: throw RequestTimeoutException(seq)
        }
    } finally {
        forget()
    }
}

/** The peer did not answer a request in time. The session goes on, and a late answer is ignored. */
class RequestTimeoutException(seq: Long) : IOException("No answer to request $seq in time")

/** The two Hellos share no protocol version. Tell the user which app to update. */
class VersionMismatchException(local: Hello, peer: Hello) : SessionException(
    "Peer speaks protocol ${peer.minProtocolVersion}..${peer.protocolVersion}, " +
        "this device ${local.minProtocolVersion}..${local.protocolVersion}",
)
