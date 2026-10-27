package dev.link2linux.android.feature.calls

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import dev.link2linux.android.core.common.NotificationIds
import dev.link2linux.android.core.designsystem.R as DesignR

/**
 * The prompt to pair with a computer over Bluetooth. Tapping it opens
 * [PairComputerActivity]. Android withdraws it once the computer's pairing
 * window is over, since pairing would fail after.
 */
internal class PairingNotifier(private val context: Context) {
    private val manager = context.getSystemService(NotificationManager::class.java)

    init {
        val channel = NotificationChannel(CHANNEL, context.getString(R.string.calls_channel_name), NotificationManager.IMPORTANCE_HIGH)
            .apply { description = context.getString(R.string.calls_channel_description) }
        manager?.createNotificationChannel(channel)
    }

    /** Shows the prompt, or returns false when the user blocked the app's notifications or this channel. */
    fun show(name: String, address: String): Boolean {
        if (!canShow()) return false
        val open = PendingIntent.getActivity(
            context,
            0,
            PairComputerActivity.intent(context, address),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = context.getString(R.string.calls_pair_text)
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(DesignR.drawable.ic_call)
            .setContentTitle(context.getString(R.string.calls_pair_title, name))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setTimeoutAfter(PAIRING_WINDOW_MILLIS)
            .setContentIntent(open)
            .build()
        manager?.notify(NotificationIds.PAIRING, notification)
        return true
    }

    private fun canShow(): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled() &&
            manager?.getNotificationChannel(CHANNEL)?.importance != NotificationManager.IMPORTANCE_NONE

    fun cancel() {
        manager?.cancel(NotificationIds.PAIRING)
    }

    private companion object {
        const val CHANNEL = "calls-pairing"

        /** How long the computer accepts pairing (docs/protocol.md, "Calls"). */
        const val PAIRING_WINDOW_MILLIS = 2 * 60_000L
    }
}
