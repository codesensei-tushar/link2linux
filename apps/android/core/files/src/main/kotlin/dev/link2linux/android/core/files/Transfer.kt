package dev.link2linux.android.core.files

/** Which way a transfer goes, seen from the phone. */
enum class Direction { INCOMING, OUTGOING }

/**
 * Names a transfer on the phone. The sender picks the id, so an incoming
 * transfer is also told apart by its desktop and by its direction.
 */
data class TransferKey(val direction: Direction, val peerId: String, val id: TransferId)

/** Why a transfer stopped before it was complete. */
enum class FailureReason {
    /** The desktop has files turned off. */
    PEER_DISABLED,

    /** The desktop lacks a permission it needs for files. */
    PEER_PERMISSION_REQUIRED,

    /** The desktop has no room for the files. */
    PEER_NO_SPACE,

    /** The desktop refused the transfer or ended it with an error. */
    PEER_ERROR,

    /** The desktop broke the protocol's rules for chunks, so the transfer could not go on. */
    PEER_INVALID,

    /** The phone has no room for the files. */
    NO_SPACE,

    /** A file could not be read. */
    UNREADABLE,

    /** A file changed size while it was being sent. */
    CHANGED,

    /** A received file did not match its hash, and was deleted. */
    DAMAGED,

    /** A received file could not be written. */
    WRITE_FAILED,

    /** The connection did not come back within an hour. */
    EXPIRED,

    /** Something unexpected went wrong on the phone. */
    INTERNAL,
}

/** Where a transfer stands. */
sealed interface TransferState {
    /** An outgoing transfer waits for its desktop to connect, or for its turn. */
    data object Waiting : TransferState

    data object Running : TransferState

    /** The connection dropped; it resumes when the desktop is back within an hour. */
    data object Interrupted : TransferState

    data object Completed : TransferState

    data class Cancelled(val byPeer: Boolean) : TransferState

    data class Failed(val reason: FailureReason) : TransferState

    val isFinished: Boolean get() = this is Completed || this is Cancelled || this is Failed
}

/** A transfer as the user sees it. */
data class Transfer(
    val key: TransferKey,
    val peer: FilePeer,
    /** The files' names, as they went out or as the desktop sent them. */
    val names: List<String>,
    val totalBytes: Long,
    /** Bytes received, or bytes the desktop confirmed. */
    val doneBytes: Long,
    val state: TransferState,
    /** The received files that are complete, under the names they got. */
    val saved: List<SavedFile>,
    /** When the user sent the files, in Unix milliseconds. */
    val createdUnixMs: Long,
)
