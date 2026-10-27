package dev.link2linux.android.feature.screen

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import dev.link2linux.android.core.common.NotificationIds
import dev.link2linux.android.core.designsystem.R as DesignR

/**
 * Holds the screen capture while a computer sees the screen, as Android
 * requires: a foreground service of the media projection type, started
 * only after the user agreed, for that one stream. Its notification names
 * the computer and stops the sharing. Once the capture is released, the
 * service stops.
 */
class ScreenCaptureService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel(CHANNEL, getString(R.string.screen_sharing_channel_name), NotificationManager.IMPORTANCE_LOW)
            .apply { description = getString(R.string.screen_sharing_channel_description) }
        getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val runtime = ScreenRuntime.from(this)
        val askId = intent?.getLongExtra(EXTRA_ASK_ID, 0) ?: 0
        val data = intent?.let { IntentCompat.getParcelableExtra(it, EXTRA_DATA, Intent::class.java) }
        // In the foreground first: Android hands out the capture only to a running service of its type.
        if (!startInForeground(intent?.getStringExtra(EXTRA_DESKTOP_NAME).orEmpty()) || data == null) {
            runtime.engine.couldNotStart(askId)
            stopSelfResult(startId)
            return START_NOT_STICKY
        }
        val projection = projection(intent.getIntExtra(EXTRA_RESULT_CODE, 0), data)
        if (projection == null) {
            runtime.engine.couldNotStart(askId)
            stopSelfResult(startId)
            return START_NOT_STICKY
        }
        // Only this start's stop ends the service: a later stream may have started it again meanwhile.
        runtime.projectionReady(askId, projection) { stopSelfResult(startId) }
        return START_NOT_STICKY
    }

    private fun startInForeground(desktopName: String): Boolean {
        val stop = PendingIntent.getBroadcast(
            this,
            0,
            ScreenActionReceiver.stopIntent(this),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val notification = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(DesignR.drawable.ic_screen_share)
            .setContentTitle(getString(R.string.screen_sharing_notification_title, desktopName))
            .setContentText(getString(R.string.screen_sharing_notification_text))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
            .addAction(0, getString(R.string.screen_stop_sharing), stop)
            .build()
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION else 0
        return try {
            ServiceCompat.startForeground(this, NotificationIds.SCREEN_SHARING, notification, type)
            true
        } catch (e: RuntimeException) {
            // SecurityException, or ForegroundServiceStartNotAllowedException from Android 12 on.
            Log.w(TAG, "Could not start the screen capture in the foreground: ${e.javaClass.simpleName}")
            false
        }
    }

    private fun projection(resultCode: Int, data: Intent): MediaProjection? = try {
        getSystemService(MediaProjectionManager::class.java)?.getMediaProjection(resultCode, data)
    } catch (e: RuntimeException) {
        // A consent used twice, or a service of the wrong type, throws.
        Log.w(TAG, "Could not take the screen capture: ${e.javaClass.simpleName}")
        null
    }

    internal companion object {
        private const val TAG = "link2linux"
        private const val CHANNEL = "screen-sharing"
        private const val EXTRA_ASK_ID = "dev.link2linux.android.screen.ASK_ID"
        private const val EXTRA_DESKTOP_NAME = "dev.link2linux.android.screen.DESKTOP_NAME"
        private const val EXTRA_RESULT_CODE = "dev.link2linux.android.screen.RESULT_CODE"
        private const val EXTRA_DATA = "dev.link2linux.android.screen.DATA"

        /** Starts the service with Android's answer, while the activity that asked is in front. */
        fun start(context: Context, askId: Long, desktopName: String, resultCode: Int, data: Intent) {
            val intent = Intent(context, ScreenCaptureService::class.java)
                .putExtra(EXTRA_ASK_ID, askId)
                .putExtra(EXTRA_DESKTOP_NAME, desktopName)
                .putExtra(EXTRA_RESULT_CODE, resultCode)
                .putExtra(EXTRA_DATA, data)
            ContextCompat.startForegroundService(context, intent)
        }
    }
}
