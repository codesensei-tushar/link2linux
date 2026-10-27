package dev.link2linux.android.feature.screen

import android.app.ActivityManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import dev.link2linux.android.core.common.NotificationIds
import dev.link2linux.android.core.screen.ScreenAsker
import dev.link2linux.android.core.screen.ScreenEngine
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import dev.link2linux.android.core.designsystem.R as DesignR

/**
 * Asks the user whether to share the screen with a computer, naming it.
 * While the app is in front, [ScreenShareActivity] asks at once; otherwise
 * a notification asks, and tapping it opens the activity, which goes
 * straight to Android's own question. Without a way to ask, as when the
 * user blocked the app's notifications, it does not ask, and the computer
 * hears that the phone cannot.
 */
internal class ScreenAsk(private val context: Context) : ScreenAsker {

    /** The question that waits for the user's answer. */
    data class Question(val askId: Long, val desktopName: String)

    private val manager = context.getSystemService(NotificationManager::class.java)
    private val _current = MutableStateFlow<Question?>(null)

    /** The question that waits for the user's answer, or null; the activity leaves once it is taken back. */
    val current: StateFlow<Question?> = _current.asStateFlow()

    init {
        val channel = NotificationChannel(CHANNEL, context.getString(R.string.screen_ask_channel_name), NotificationManager.IMPORTANCE_HIGH)
            .apply { description = context.getString(R.string.screen_ask_channel_description) }
        manager?.createNotificationChannel(channel)
        // Questions do not outlive the process, so one an earlier run left asks nothing.
        manager?.cancel(NotificationIds.SCREEN_ASK)
    }

    override fun ask(askId: Long, desktopName: String): Boolean {
        val question = Question(askId, desktopName)
        if (inFront()) {
            _current.value = question
            try {
                context.startActivity(ScreenShareActivity.intent(context, question, confirmFirst = true).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return true
            } catch (_: ActivityNotFoundException) {
                // Not expected for the app's own activity; the notification may still ask.
            }
        }
        if (!canNotify()) {
            _current.value = null
            return false
        }
        _current.value = question
        manager?.notify(NotificationIds.SCREEN_ASK, notification(question))
        return true
    }

    override fun withdraw(askId: Long) {
        var withdrawn = false
        _current.update { current ->
            if (current?.askId == askId) {
                withdrawn = true
                null
            } else {
                current
            }
        }
        if (withdrawn) manager?.cancel(NotificationIds.SCREEN_ASK)
    }

    private fun notification(question: Question) = NotificationCompat.Builder(context, CHANNEL).run {
        val code = question.askId.toInt()
        val share = PendingIntent.getActivity(
            context,
            code,
            ScreenShareActivity.intent(context, question, confirmFirst = false),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val decline = PendingIntent.getBroadcast(
            context,
            code,
            ScreenActionReceiver.declineIntent(context, question.askId),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        setSmallIcon(DesignR.drawable.ic_screen_share)
        setContentTitle(context.getString(R.string.screen_ask_title, question.desktopName))
        setContentText(context.getString(R.string.screen_ask_text))
        setPriority(NotificationCompat.PRIORITY_HIGH)
        setAutoCancel(true)
        // Later than the engine's own minute. The engine withdraws the question then, and cancelling
        // the notification does not run its decline action. A timeout of the same length would: the
        // system removes the notification and runs that action, so a user who simply did not answer
        // would be reported as having declined. This only tidies up after a lost process.
        setTimeoutAfter(ScreenEngine.ANSWER_MILLIS + NOTIFICATION_TIMEOUT_MARGIN_MILLIS)
        setContentIntent(share)
        // Swiping the question away declines it.
        setDeleteIntent(decline)
        addAction(0, context.getString(R.string.screen_ask_share), share)
        addAction(0, context.getString(R.string.screen_ask_decline), decline)
        build()
    }

    private fun canNotify(): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled() &&
            manager?.getNotificationChannel(CHANNEL)?.importance != NotificationManager.IMPORTANCE_NONE

    /** Whether one of the app's activities is in front, so that the app may start another. */
    private fun inFront(): Boolean {
        val state = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(state)
        return state.importance == ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }

    private companion object {
        const val CHANNEL = "screen-ask"

        /** How long after the engine's minute the notification may stay if the process is gone. */
        const val NOTIFICATION_TIMEOUT_MARGIN_MILLIS = 30_000L
    }
}
