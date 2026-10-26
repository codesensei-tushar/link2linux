package dev.link2linux.android.core.clipboard

import dev.link2linux.protocol.v1.ClipboardPush
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ErrorReply
import dev.link2linux.protocol.v1.errorReply

/*
 * The seam between the clipboard engine and the session. The session
 * attaches a [ClipboardPeer] for each desktop where the feature is active,
 * tells the engine what the desktop's `FeatureStatus` says, and passes each
 * push the desktop sends to [ClipboardEngine.receive], sending back the
 * [ClipboardReply] with the request's `seq` as `reply_to`.
 */

/** A connected desktop where the feature is active. */
interface ClipboardPeer {
    /** Names the desktop, the same in each of its sessions. */
    val desktopId: String

    /** The desktop's name, for the user. */
    val name: String

    /**
     * Sends [push] as a request and waits for the answer, at most the
     * protocol's 30 seconds. Throws when the connection fails first.
     */
    suspend fun push(push: ClipboardPush): PushAnswer
}

/** A desktop's answer to a push. */
sealed interface PushAnswer {
    data object Ack : PushAnswer

    data class Error(val code: ErrorCode) : PushAnswer

    /** None arrived in time: the push failed, and a late answer is ignored. */
    data object NoAnswer : PushAnswer
}

/** The phone's answer to a desktop's push. */
sealed interface ClipboardReply {
    data object Ack : ClipboardReply

    data class Error(val message: ErrorReply) : ClipboardReply {
        val code: ErrorCode get() = message.code
    }
}

internal fun failure(code: ErrorCode, detail: String) = ClipboardReply.Error(
    errorReply {
        this.code = code
        this.detail = detail
    },
)
