package dev.link2linux.android.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import dagger.hilt.android.AndroidEntryPoint
import dev.link2linux.android.MainActivity
import dev.link2linux.android.R
import dev.link2linux.android.core.data.DesktopListener
import dev.link2linux.android.core.discovery.ConnectionSwitch
import dev.link2linux.android.core.discovery.DiscoveryController
import dev.link2linux.android.core.discovery.android.ConnectivityLocalNetworks
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import javax.inject.Inject

/**
 * The foreground service that keeps the phone findable and connected
 * (type `connectedDevice`). It runs while the phone has a pairing or a
 * pairing is under way, and it keeps running when the phone leaves Wi-Fi
 * and Ethernet: Android lets an app start a foreground service from the
 * background only in a few cases, so the service never stops and restarts
 * with the network. Discovery pauses by itself while there is no network
 * and resumes when one returns.
 *
 * It is started when the app is opened, and from the background only by
 * the companion-device service ([DesktopPresenceService]). Running it on a
 * device is unverified.
 */
@AndroidEntryPoint
class ConnectionService : Service() {

    @Inject lateinit var discovery: DiscoveryController
    @Inject lateinit var networks: ConnectivityLocalNetworks
    @Inject lateinit var listener: DesktopListener
    @Inject lateinit var connectionSwitch: ConnectionSwitch

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    override fun onCreate() {
        super.onCreate()
        createChannel()
        // connectedDevice needs one of the permissions Android names for it;
        // CHANGE_WIFI_MULTICAST_STATE is declared for that and for pairing.
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            notification(),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
        networks.start()
        discovery.start()
        // Listening for as long as the service runs lets paired desktops reconnect outside a pairing window.
        scope.launch { listener.start() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                // "Turn off" is the user's choice: it stays off until they turn it on in the app.
                connectionSwitch.setEnabled(false)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_DESKTOP_NEARBY -> discovery.onDesktopNearby()
        }
        return START_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        networks.stop()
        runBlocking {
            listener.stop()
            discovery.stop()
        }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                getString(R.string.service_channel_name),
                NotificationManager.IMPORTANCE_MIN,
            ).apply { description = getString(R.string.service_channel_description) },
        )
    }

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            stopIntent(this),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentTitle(getString(R.string.service_notification_title))
            .setContentText(getString(R.string.service_notification_text))
            .setContentIntent(open)
            .addAction(0, getString(R.string.service_notification_stop), stop)
            .setOngoing(true)
            .setSilent(true)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "connection"
        private const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "dev.link2linux.android.action.STOP"
        const val ACTION_DESKTOP_NEARBY = "dev.link2linux.android.action.DESKTOP_NEARBY"

        private fun stopIntent(context: Context) =
            Intent(context, ConnectionService::class.java).setAction(ACTION_STOP)

        /** Starts the service. Call it only where Android allows a foreground service start. */
        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, ConnectionService::class.java))
        }

        /** The intent the companion-device service sends when the desktop appears. */
        fun desktopNearby(context: Context) =
            Intent(context, ConnectionService::class.java).setAction(ACTION_DESKTOP_NEARBY)
    }
}
