package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.Pong
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.io.IOException

/**
 * One session with a paired peer over an established [Transport].
 *
 * Both sides send `Hello` as their first frame (seq 1), then read the
 * peer's. A peer that speaks another protocol major version, or sends
 * anything else first, is refused. Afterwards [incoming] delivers feature
 * messages and answers `Ping` with `Pong`. The peer's sequence numbers must
 * strictly increase.
 *
 * Skeleton for now: it does not send keep-alive pings or time out idle peers.
 */
class Session(
    private val transport: Transport,
    private val codec: FrameCodec = FrameCodec(),
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : Closeable {

    private val writeLock = Mutex()
    private var nextSeq = 1L // guarded by writeLock
    private var lastPeerSeq = 0L // touched only by the single reader

    private val _state = MutableStateFlow<SessionState>(SessionState.Handshaking)
    val state: StateFlow<SessionState> = _state.asStateFlow()

    /** Exchanges `Hello`s and returns the peer's. Closes the session on failure. */
    suspend fun handshake(local: Hello): Hello = try {
        sendPayload { hello = local }
        val first = receive() ?: throw SessionException("Peer closed the stream before its Hello")
        if (!first.hasHello()) throw SessionException("Expected Hello first, got ${first.payloadCase}")
        val peer = first.hello
        if (peer.protocolVersion != local.protocolVersion) {
            throw SessionException(
                "Peer speaks protocol ${peer.protocolVersion}, this device ${local.protocolVersion}",
            )
        }
        _state.value = SessionState.Established(peer)
        peer
    } catch (e: Exception) {
        close()
        throw e
    }

    /** Sends one feature message and returns its sequence number. */
    suspend fun send(fill: Envelope.Builder.() -> Unit): Long = sendPayload(fill)

    /** Feature messages from the peer until it closes the stream. */
    fun incoming(): Flow<Envelope> = flow {
        while (true) {
            val envelope = receive() ?: break
            if (envelope.hasPing()) {
                sendPayload { setPong(Pong.newBuilder().setNonce(envelope.ping.nonce)) }
            } else {
                emit(envelope)
            }
        }
        close()
    }

    override fun close() {
        _state.value = SessionState.Closed
        transport.close()
    }

    private suspend fun sendPayload(fill: Envelope.Builder.() -> Unit): Long = withContext(ioDispatcher) {
        writeLock.withLock {
            val seq = nextSeq++
            codec.write(Envelope.newBuilder().setSeq(seq).apply(fill).build(), transport.output)
            transport.output.flush()
            seq
        }
    }

    private suspend fun receive(): Envelope? = withContext(ioDispatcher) {
        codec.read(transport.input)?.also { envelope ->
            if (envelope.seq <= lastPeerSeq) {
                throw SessionException("Peer sent seq ${envelope.seq} after $lastPeerSeq")
            }
            lastPeerSeq = envelope.seq
        }
    }
}

sealed interface SessionState {
    data object Handshaking : SessionState
    data class Established(val peer: Hello) : SessionState
    data object Closed : SessionState
}

class SessionException(message: String) : IOException(message)
