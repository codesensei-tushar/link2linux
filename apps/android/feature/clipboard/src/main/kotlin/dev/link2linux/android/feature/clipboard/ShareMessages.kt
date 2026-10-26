package dev.link2linux.android.feature.clipboard

import androidx.annotation.StringRes
import dev.link2linux.android.core.clipboard.DesktopOutcome
import dev.link2linux.android.core.clipboard.PushOutcome
import dev.link2linux.android.core.clipboard.ShareResult

/** A short message for the user: a string resource and its arguments. */
internal data class Message(@StringRes val id: Int, val args: List<String> = emptyList())

/** What to tell the user after a share; null when there is nothing to say, as for a share a newer one replaced. */
internal fun messageFor(result: ShareResult): Message? = when (result) {
    ShareResult.Disabled -> Message(R.string.clipboard_result_off_here)
    ShareResult.NoDesktop -> Message(R.string.clipboard_result_no_computer)
    ShareResult.Empty -> Message(R.string.clipboard_result_nothing)
    ShareResult.TooLarge -> Message(R.string.clipboard_result_too_long)
    is ShareResult.Pushed -> messageFor(result.outcomes)
}

private fun messageFor(outcomes: List<DesktopOutcome>): Message? {
    fun names(of: List<DesktopOutcome>) = of.joinToString(", ") { it.name }
    val sent = outcomes.filter { it.outcome == PushOutcome.SENT }
    val notSent = outcomes.filter { it.outcome == PushOutcome.TURNED_OFF || it.outcome == PushOutcome.FAILED }
    return when {
        sent.isNotEmpty() && notSent.isEmpty() -> Message(R.string.clipboard_result_sent, listOf(names(sent)))
        sent.isNotEmpty() -> Message(R.string.clipboard_result_sent_partly, listOf(names(sent), names(notSent)))
        // Replaced everywhere: the newer share has its own message.
        notSent.isEmpty() -> null
        notSent.all { it.outcome == PushOutcome.TURNED_OFF } -> Message(R.string.clipboard_result_off_there, listOf(names(notSent)))
        else -> Message(R.string.clipboard_result_failed, listOf(names(notSent)))
    }
}
