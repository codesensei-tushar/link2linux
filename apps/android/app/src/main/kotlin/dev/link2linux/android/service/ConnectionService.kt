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
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import dagger.hilt.android.AndroidEntryPoint
import dev.link2linux.android.MainActivity
import dev.link2linux.android.R
import dev.link2linux.android.core.common.NotificationIds
import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.DesktopListener
import dev.link2linux.android.core.data.ConnectionPolicy
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.data.connectedIn
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.discovery.ConnectionSwitch
import dev.link2linux.android.core.discovery.DiscoveryController
import dev.link2linux.android.core.discovery.android.ConnectivityLocalNetworks
import dev.link2linux.android.feature.clipboard.ClipboardRuntime
import dev.link2linux.android.feature.clipboard.ClipboardSendActivity
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import javax.inject.Inject
import dev.link2linux.android.feature.clipboard.R as ClipboardR

/**
 * The foreground service that keeps the phone findable and connected
 * (type `connectedDevice`). It runs while the user's switch is on and the
 * phone has a pairing or a pairing is under way ([ConnectionPolicy.shouldRun]),
 * and stops itself otherwise. It keeps running when the phone leaves Wi-Fi
 * and Ethernet: Android lets an app start a foreground service from the
 * background only in a few cases, so the service never stops and restarts
 * with the network. The listener closes and discovery pauses while there is
 * no local network, and both resume when one returns.
 *
 * The app starts it while it is visible and the policy says so, and from
 * the background only the companion-device service ([DesktopPresenceService])
 * does. Running it on a device is unverified.
 *
 * Its notification offers "Send clipboard" while that feature is on and a
 * connected computer takes the clipboard.
 */
@AndroidEntryPoint
class ConnectionService : Service() {

    @Inject lateinit var discovery: DiscoveryController
    @Inject lateinit var networks: ConnectivityLocalNetworks
    @Inject lateinit var listener: DesktopListener
    @Inject lateinit var connectionSwitch: ConnectionSwitch
    @Inject lateinit var pairedDevices: PairedDeviceRepository
    @Inject lateinit var pairing: PairingController
    @Inject lateinit var connections: DesktopConnections

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + loggingUncaught("the connection service"))

    /** What the notification shows: the connected computers, and whether one of them takes the clipboard. */
    private data class Shown(val connectedTo: List<String> = emptyList(), val clipboard: Boolean = false)

    // Read and written on the main thread only, where the notification is posted too.
    private var destroyed = false
    private var shown = Shown()

    override fun onCreate() {
        super.onCreate()
        createChannel()
        // connectedDevice needs one of the permissions Android names for it;
        // CHANGE_WIFI_MULTICAST_STATE is declared for that and for pairing.
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            notification(shown),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
        networks.start()
        discovery.start()
        // Nothing paired and nothing under way, or the user turned it off: nothing to keep running.
        scope.launch {
            combine(pairedDevices.pairedDevices, pairing.state, connectionSwitch.enabled, ConnectionPolicy::shouldRun)
                .distinctUntilChanged()
                .collect { run -> if (!run) withContext(Dispatchers.Main) { stopSelf() } }
        }
        // Listening while a desktop is paired lets it reconnect outside a pairing window.
        // Off a local network the listener closes, so nothing listens on mobile data.
        scope.launch {
            val onLocalNetwork = networks.networks.map { it.isNotEmpty() }
            combine(pairedDevices.pairedDevices, pairing.state, connectionSwitch.enabled, onLocalNetwork, ConnectionPolicy::shouldListen)
                .distinctUntilChanged()
                .collectLatest { listen -> if (listen) listen() else runCatching { listener.stop() } }
        }
        scope.launch {
            val clipboard = ClipboardRuntime.get(this@ConnectionService).canSend
            combine(pairedDevices.pairedDevices, connections.connected, clipboard) { devices, connected, canSendClipboard ->
                Shown(devices.connectedIn(connected).map { it.device.name }, canSendClipboard)
            }.distinctUntilChanged().collect { next ->
                withContext(Dispatchers.Main) {
                    shown = next
                    if (!destroyed) getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(next))
                }
            }
        }
        // A computer the phone was unpaired from must not keep waking it.
        scope.launch { CompanionAssociations(this@ConnectionService).follow(pairedDevices.pairedDevices) }
    }

    /** Starts the listener, and tries again while it cannot, for example while its port is briefly taken. */
    private suspend fun listen() {
        while (true) {
            try {
                listener.start()
                return
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "Could not start listening", e)
                delay(LISTEN_RETRY_MILLIS)
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            // "Turn off" is the user's choice: it stays off until they turn it on in the app.
            pairing.cancel()
            connectionSwitch.setEnabled(false)
            stopSelf()
            return START_NOT_STICKY
        }
        if (!connectionSwitch.enabled.value) {
            // Turned off since this start was asked for, or a restart after the process died.
            stopSelf()
            return START_NOT_STICKY
        }
        if (intent?.action == ACTION_DESKTOP_NEARBY) discovery.onDesktopNearby()
        return START_STICKY
    }

    override fun onDestroy() {
        destroyed = true
        scope.cancel()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
        networks.stop()
        runBlocking {
            runCatching { listener.stop() }
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

    private fun notification(shown: Shown): Notification {
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
        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentTitle(getString(R.string.service_notification_title))
            .setContentText(
                if (shown.connectedTo.isEmpty()) {
                    getString(R.string.service_notification_text)
                } else {
                    getString(R.string.service_notification_connected, shown.connectedTo.joinToString(", "))
                },
            )
            .setContentIntent(open)
        if (shown.clipboard) {
            builder.addAction(
                0,
                getString(ClipboardR.string.clipboard_send_action),
                ClipboardSendActivity.pendingIntent(this, SEND_CLIPBOARD_REQUEST),
            )
        }
        return builder
            .addAction(0, getString(R.string.service_notification_stop), stop)
            .setOngoing(true)
            .setSilent(true)
            .build()
    }

    companion object {
        private const val TAG = "link2linux"
        private const val CHANNEL_ID = "connection"
        private const val NOTIFICATION_ID = NotificationIds.CONNECTION
        private const val LISTEN_RETRY_MILLIS = 10_000L
        private const val SEND_CLIPBOARD_REQUEST = 2
        const val ACTION_STOP = "dev.link2linux.android.action.STOP"
        const val ACTION_DESKTOP_NEARBY = "dev.link2linux.android.action.DESKTOP_NEARBY"

        private fun stopIntent(context: Context) =
            Intent(context, ConnectionService::class.java).setAction(ACTION_STOP)

        /**
         * Starts the service. Call it only where Android allows a foreground
         * service start, and while [ConnectionPolicy.shouldRun] holds: it
         * stops itself otherwise.
         */
        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, ConnectionService::class.java))
        }

        /** The intent the companion-device service sends when the desktop appears. */
        fun desktopNearby(context: Context) =
            Intent(context, ConnectionService::class.java).setAction(ACTION_DESKTOP_NEARBY)
    }
}
