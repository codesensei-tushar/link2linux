package dev.link2linux.android.core.files

import com.google.protobuf.ByteString
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ErrorReply
import dev.link2linux.protocol.v1.FileAccept
import dev.link2linux.protocol.v1.FileCancel
import dev.link2linux.protocol.v1.FileChunk
import dev.link2linux.protocol.v1.FileOffer
import dev.link2linux.protocol.v1.errorReply
import kotlinx.coroutines.flow.Flow
import java.io.IOException
import java.security.SecureRandom

/*
 * The seam between the files engine and the session. The session implements
 * [FileLink] (it owns the envelopes, sequence numbers and answer timeouts)
 * and calls [FileEngine.handle] for each request of the desktop, sending the
 * returned answer with the request's `seq` as `reply_to`.
 */

/** 16 random bytes naming a transfer, chosen by its sender. */
@JvmInline
value class TransferId(val bytes: ByteString) {
    override fun toString(): String = bytes.toByteArray().joinToString("") { "%02x".format(it) }

    companion object {
        const val SIZE = 16

        fun random(random: SecureRandom): TransferId = TransferId(ByteString.copyFrom(ByteArray(SIZE).also(random::nextBytes)))
    }
}

/** A paired desktop. [id] stays the same as long as the pairing does; [name] is for the user. */
data class FilePeer(val id: String, val name: String)

/** The desktop of a pairing record, named by the id it was paired with, which a key rotation keeps. */
fun PairedDevice.toFilePeer(): FilePeer = FilePeer(device.id.value, device.name)

/** A request of the files feature, either way. */
sealed interface FileRequest {
    data class Offer(val message: FileOffer) : FileRequest

    data class Chunk(val message: FileChunk) : FileRequest

    data class Cancel(val message: FileCancel) : FileRequest
}

/** The answer to a [FileRequest]. */
sealed interface FileAnswer {
    data class Accept(val message: FileAccept) : FileAnswer

    /** `Ack`. */
    data object Ack : FileAnswer

    data class Error(val message: ErrorReply) : FileAnswer {
        val code: ErrorCode get() = message.code
    }
}

/** One session with a desktop, as the engine uses it. */
interface FileLink {
    val peer: FilePeer

    /**
     * Whether the desktop's latest `FeatureStatus` says files are `READY`
     * there, its current value first. Offers go out only while it does.
     */
    val peerReady: Flow<Boolean>

    /**
     * Writes [request] and returns once it is written, with a handle that
     * waits for its answer, so several requests can be on their way at once.
     * Writes keep the order of the calls.
     *
     * @throws IOException when the connection is gone.
     */
    suspend fun send(request: FileRequest): PendingAnswer
}

/** The answer to one request, on its way. */
fun interface PendingAnswer {
    /**
     * @throws AnswerTimeoutException when no answer came within the time a
     *   request may wait (30 seconds).
     * @throws IOException when the session ended first.
     */
    suspend fun await(): FileAnswer
}

/** A request got no answer in time while the session stayed up. */
class AnswerTimeoutException(message: String) : IOException(message)

internal fun failure(code: ErrorCode, detail: String = "", permission: String = "", retryAfterMillis: Int = 0) =
    FileAnswer.Error(
        errorReply {
            this.code = code
            this.detail = detail.take(1024)
            this.permission = permission
            this.retryAfterMs = retryAfterMillis
        },
    )
