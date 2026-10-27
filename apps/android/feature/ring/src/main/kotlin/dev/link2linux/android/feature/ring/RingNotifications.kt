package dev.link2linux.android.feature.ring

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import androidx.core.app.NotificationCompat
import dev.link2linux.android.core.common.NotificationIds
import dev.link2linux.android.core.ring.RingNotifier
import dev.link2linux.android.core.designsystem.R as DesignR

/**
 * The notification shown while the phone rings: it names the computer that
 * looks for the phone, and tapping it, its Stop button or swiping it away
 * stops the ringing. Its channel makes no sound of its own, since the
 * ringtone plays already.
 */
internal class RingNotifications(private val context: Context) : RingNotifier {
    private val manager = context.getSystemService(NotificationManager::class.java)

    init {
        val channel = NotificationChannel(CHANNEL, context.getString(R.string.ring_channel_name), NotificationManager.IMPORTANCE_HIGH).apply {
            description = context.getString(R.string.ring_channel_description)
            setSound(null, null)
            enableVibration(false)
        }
        manager?.createNotificationChannel(channel)
        // Ringing does not outlive the process, so a notification an earlier run left stops nothing.
        cancel()
    }

    override fun show(desktopName: String) {
        val stop = PendingIntent.getBroadcast(
            context,
            0,
            RingStopReceiver.stopIntent(context),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(DesignR.drawable.ic_ring)
            .setContentTitle(context.getString(R.string.ring_notification_title))
            .setContentText(context.getString(R.string.ring_notification_text, desktopName))
            .setCategory(NotificationCompat.CATEGORY_ALARM)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(stop)
            .setDeleteIntent(stop)
            .addAction(0, context.getString(R.string.ring_stop), stop)
            .build()
        // Without the notification permission nothing shows; the Ring screen can still stop it.
        manager?.notify(NotificationIds.RING, notification)
    }

    override fun cancel() {
        manager?.cancel(NotificationIds.RING)
    }

    private companion object {
        const val CHANNEL = "ring"
    }
}
