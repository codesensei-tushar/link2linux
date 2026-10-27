package dev.link2linux.android.feature.calls

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import androidx.core.app.NotificationCompat
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

    fun show(name: String, address: String) {
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
        // Without the notification permission nothing shows; the Calls screen opens Bluetooth settings.
        manager?.notify(ID, notification)
    }

    fun cancel() {
        manager?.cancel(ID)
    }

    private companion object {
        const val CHANNEL = "calls-pairing"
        const val ID = 1

        /** How long the computer accepts pairing (docs/protocol.md, "Calls"). */
        const val PAIRING_WINDOW_MILLIS = 2 * 60_000L
    }
}
