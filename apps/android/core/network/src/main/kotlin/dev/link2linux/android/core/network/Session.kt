package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Goodbye
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.Pong
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean

/**
 * One connection's worth of envelopes over an established [Transport].
 *
 * Both sides send `Hello` as their first frame (seq 1), then read the
 * peer's, and use the highest protocol version both ranges contain. Each
 * peer's sequence numbers must go up by exactly one. [next] and [incoming]
 * answer `Ping` with `Pong` themselves. A protocol error sends `Goodbye`
 * with [GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR] and closes.
 *
 * Liveness is the transport's: on the phone, a read that waits longer than
 * the idle timeout fails, and that ends the session. Writes have their own
 * deadline, [writeTimeoutMillis]: a peer that stops reading would otherwise
 * block every sender of this session.
 */
class Session(
    private val transport: Transport,
    private val codec: FrameCodec = FrameCodec(),
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    /** How long writing one envelope may take before the connection is closed. */
    private val writeTimeoutMillis: Long = WRITE_TIMEOUT_MILLIS,
) : Closeable {

    private val writeLock = Mutex()
    private var nextSeq = 1L // guarded by writeLock
    private var lastPeerSeq = 0L // touched only by the single reader

    private val _state = MutableStateFlow<SessionState>(SessionState.Handshaking)
    val state: StateFlow<SessionState> = _state.asStateFlow()

    /**
     * Exchanges `Hello`s and returns the peer's. Closes the session on
     * failure, and when the exchange takes longer than [timeoutMillis]; when
     * the version ranges do not overlap it first sends `Goodbye` with
     * [GoodbyeReason.GOODBYE_REASON_VERSION_MISMATCH].
     */
    suspend fun handshake(local: Hello, timeoutMillis: Long = HANDSHAKE_TIMEOUT_MILLIS): Hello = try {
        closingAfter(timeoutMillis, "The Hello exchange") {
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

    /** Sends one envelope and returns its sequence number. */
    suspend fun send(fill: Envelope.Builder.() -> Unit): Long = withContext(ioDispatcher) {
        writeLock.withLock {
            val seq = nextSeq++
            val envelope = Envelope.newBuilder().apply(fill).setSeq(seq).build()
            closingAfter(writeTimeoutMillis, "Writing envelope $seq") {
                codec.write(envelope, transport.output)
                transport.output.flush()
            }
            seq
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
                envelope.hasPing() -> send { setPong(Pong.newBuilder().setNonce(envelope.ping.nonce)) }
                envelope.hasHello() -> protocolError("Second Hello")
                else -> return envelope
            }
        }
    }

    /** The peer's envelopes, as from [next], until it closes the stream; then the session closes. */
    fun incoming(): Flow<Envelope> = flow {
        while (true) emit(next() ?: break)
        close()
    }

    /** Sends `Goodbye` and closes the connection. The send is best effort. */
    suspend fun goodbye(reason: GoodbyeReason, detail: String = "") {
        runCatching { send { goodbye = Goodbye.newBuilder().setReason(reason).setDetail(detail).build() } }
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
        val envelope = withContext(ioDispatcher) { codec.read(transport.input) } ?: return null
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

/** The two Hellos share no protocol version. Tell the user which app to update. */
class VersionMismatchException(local: Hello, peer: Hello) : SessionException(
    "Peer speaks protocol ${peer.minProtocolVersion}..${peer.protocolVersion}, " +
        "this device ${local.minProtocolVersion}..${local.protocolVersion}",
)
