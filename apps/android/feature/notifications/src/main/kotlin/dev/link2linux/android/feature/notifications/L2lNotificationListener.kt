package dev.link2linux.android.feature.notifications

import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification

/**
 * The system binds this service once the user grants notification access. It
 * only forwards the platform's callbacks to [NotificationsRuntime], which
 * decides what is shared.
 */
class L2lNotificationListener : NotificationListenerService() {
    private val runtime get() = NotificationsRuntime.get(this)

    override fun onListenerConnected() = runtime.onListenerConnected(this)

    override fun onListenerDisconnected() = runtime.onListenerDisconnected()

    override fun onNotificationPosted(sbn: StatusBarNotification, rankingMap: RankingMap) =
        runtime.onPosted(sbn, rankingMap)

    override fun onNotificationRemoved(sbn: StatusBarNotification, rankingMap: RankingMap, reason: Int) =
        runtime.onRemoved(sbn, reason)
}
