package dev.link2linux.android.feature.files

import android.content.res.Resources
import androidx.annotation.PluralsRes
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import dev.link2linux.android.core.files.Direction
import dev.link2linux.android.core.files.FailureReason
import dev.link2linux.android.core.files.Transfer
import dev.link2linux.android.core.files.TransferState

/** A string resource and its arguments, shown by a screen or by a notification. */
internal sealed interface Text {
    data class Res(@StringRes val id: Int, val args: List<Any> = emptyList()) : Text

    data class Plural(@PluralsRes val id: Int, val count: Int, val args: List<Any> = emptyList()) : Text

    fun resolve(resources: Resources): String = when (this) {
        is Res -> resources.getString(id, *args.toTypedArray())
        is Plural -> resources.getQuantityString(id, count, *args.toTypedArray())
    }
}

@Composable
internal fun Text.resolve(): String = when (this) {
    is Text.Res -> stringResource(id, *args.toTypedArray())
    is Text.Plural -> pluralStringResource(id, count, *args.toTypedArray())
}

/** What the user reads about a transfer. */
internal object TransferText {

    /** The files' names: the first, and how many more. */
    fun names(transfer: Transfer): Text {
        val first = transfer.saved.firstOrNull()?.name ?: transfer.names.firstOrNull().orEmpty()
        val more = transfer.names.size - 1
        return if (more <= 0) Text.Res(R.string.files_name_only, listOf(first)) else Text.Plural(R.plurals.files_names_more, more, listOf(first, more))
    }

    /** Where it stands, naming its desktop. */
    fun status(transfer: Transfer): Text {
        val outgoing = transfer.key.direction == Direction.OUTGOING
        val desktop = transfer.peer.name
        val id = when (val state = transfer.state) {
            TransferState.Waiting -> R.string.files_state_waiting
            TransferState.Running -> if (outgoing) R.string.files_state_sending else R.string.files_state_receiving
            TransferState.Interrupted -> R.string.files_state_interrupted
            TransferState.Completed -> if (outgoing) R.string.files_state_sent else R.string.files_state_received
            is TransferState.Cancelled -> if (state.byPeer) R.string.files_state_cancelled_by_peer else R.string.files_state_cancelled
            is TransferState.Failed -> if (outgoing) R.string.files_state_failed_sending else R.string.files_state_failed_receiving
        }
        return Text.Res(id, listOf(desktop))
    }

    /** Why it failed, or null when it did not. */
    fun reason(transfer: Transfer): Text? {
        val reason = (transfer.state as? TransferState.Failed)?.reason ?: return null
        return Text.Res(
            when (reason) {
                FailureReason.PEER_DISABLED -> R.string.files_reason_peer_disabled
                FailureReason.PEER_PERMISSION_REQUIRED -> R.string.files_reason_peer_permission
                FailureReason.PEER_NO_SPACE -> R.string.files_reason_peer_no_space
                FailureReason.PEER_ERROR -> R.string.files_reason_peer_error
                FailureReason.PEER_INVALID -> R.string.files_reason_peer_invalid
                FailureReason.NO_SPACE -> R.string.files_reason_no_space
                FailureReason.UNREADABLE -> R.string.files_reason_unreadable
                FailureReason.CHANGED -> R.string.files_reason_changed
                FailureReason.DAMAGED -> R.string.files_reason_damaged
                FailureReason.WRITE_FAILED -> R.string.files_reason_write_failed
                FailureReason.EXPIRED -> R.string.files_reason_expired
                FailureReason.INTERNAL -> R.string.files_reason_internal
            },
        )
    }

    /** Progress in thousandths, for a progress bar. */
    fun permille(transfer: Transfer): Int =
        if (transfer.totalBytes <= 0) 0 else (transfer.doneBytes.toDouble() / transfer.totalBytes * 1000).toInt().coerceIn(0, 1000)
}
