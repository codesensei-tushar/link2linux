package dev.link2linux.android.feature.notifications

import android.app.ActivityOptions
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import dev.link2linux.android.core.notifications.ActionInvoker
import dev.link2linux.android.core.notifications.ActionOutcome

/**
 * Fires a notification action's PendingIntent. For a reply it puts the text
 * into the action's RemoteInput the way the system's own reply field does.
 *
 * Unverified without a device: how each messaging app reacts to a reply sent
 * this way, and whether the background-start opt-in is honored.
 */
internal class PendingIntentInvoker(
    private val context: Context,
    private val intent: PendingIntent,
    private val remoteInput: RemoteInput?,
) : ActionInvoker {
    override fun invoke(replyText: String?, allowBackgroundStart: Boolean): ActionOutcome {
        return try {
            val fill = if (remoteInput != null && replyText != null) replyIntent(remoteInput, replyText) else null
            intent.send(context, 0, fill, null, null, null, sendOptions(allowBackgroundStart))
            ActionOutcome.SENT
        } catch (_: PendingIntent.CanceledException) {
            ActionOutcome.CANCELED
        } catch (_: SecurityException) {
            ActionOutcome.BLOCKED
        } catch (_: RuntimeException) {
            ActionOutcome.FAILED
        }
    }

    private fun replyIntent(input: RemoteInput, text: String): Intent {
        val results = Bundle().apply { putCharSequence(input.resultKey, text) }
        val fill = Intent()
        RemoteInput.addResultsToIntent(arrayOf(input), fill, results)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            val fromChoice = input.choices?.any { it.toString() == text } == true
            RemoteInput.setResultsSource(
                fill,
                if (fromChoice) RemoteInput.SOURCE_CHOICE else RemoteInput.SOURCE_FREE_FORM_INPUT,
            )
        }
        return fill
    }

    /** Opts in to starting an activity from the background, which Android 14 and later require of the sender. */
    private fun sendOptions(allowBackgroundStart: Boolean): Bundle? {
        if (!allowBackgroundStart || Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return null
        @Suppress("DEPRECATION")
        val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
            ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOW_ALWAYS
        } else {
            ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED
        }
        return ActivityOptions.makeBasic().setPendingIntentBackgroundActivityStartMode(mode).toBundle()
    }
}
