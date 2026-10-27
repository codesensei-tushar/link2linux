package dev.link2linux.android.feature.files

import android.app.DownloadManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.text.format.Formatter
import androidx.core.app.NotificationCompat
import androidx.core.net.toUri
import dev.link2linux.android.core.common.NotificationIds
import dev.link2linux.android.core.files.Direction
import dev.link2linux.android.core.files.FileEngine
import dev.link2linux.android.core.files.Transfer
import dev.link2linux.android.core.files.TransferKey
import dev.link2linux.android.core.files.TransferState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Tells the user about transfers: a progress notification with Cancel
 * while one runs, and one when it ends. A received file is never opened by
 * itself; its notification has Open, which hands it to the app the user
 * picks.
 */
internal class TransferNotifier(private val context: Context, private val engine: FileEngine, private val scope: CoroutineScope) {
    private val manager = context.getSystemService(NotificationManager::class.java)

    // Touched only by the collector.
    private val showing = HashSet<TransferKey>()
    private val announced = HashSet<TransferKey>()

    fun start() {
        createChannels()
        clearStale()
        scope.launch {
            engine.transfers.collect { transfers ->
                render(transfers)
                // The list changes with every chunk; a few updates a second are plenty.
                delay(UPDATE_MILLIS)
            }
        }
    }

    /**
     * Transfers do not outlive the process, so a progress notification an
     * earlier run left (it was killed mid-transfer) only shows something
     * that stopped, with a Cancel that reaches nothing. The ones that tell
     * how a transfer ended stay.
     */
    private fun clearStale() {
        runCatching { manager.activeNotifications }.getOrDefault(emptyArray())
            .filter { it.tag == TAG && it.notification.flags and Notification.FLAG_ONGOING_EVENT != 0 }
            .forEach { manager.cancel(TAG, it.id) }
    }

    private fun render(transfers: List<Transfer>) {
        val present = transfers.mapTo(HashSet()) { it.key }
        for (transfer in transfers) {
            val id = notificationId(transfer.key)
            if (!transfer.state.isFinished) {
                manager.notify(TAG, id, progress(transfer))
                showing += transfer.key
                announced -= transfer.key
            } else if (announced.add(transfer.key)) {
                val ended = ended(transfer)
                if (ended != null) manager.notify(TAG, id, ended) else manager.cancel(TAG, id)
                showing -= transfer.key
            }
        }
        showing.filter { it !in present }.forEach {
            manager.cancel(TAG, notificationId(it))
            showing -= it
        }
        announced.retainAll(present)
    }

    private fun progress(transfer: Transfer): Notification {
        val resources = context.resources
        val incoming = transfer.key.direction == Direction.INCOMING
        val done = Formatter.formatShortFileSize(context, transfer.doneBytes)
        val total = Formatter.formatShortFileSize(context, transfer.totalBytes)
        return NotificationCompat.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(if (incoming) R.drawable.ic_file_download else R.drawable.ic_file_upload)
            .setContentTitle(TransferText.status(transfer).resolve(resources))
            .setContentText(TransferText.names(transfer).resolve(resources))
            .setSubText(resources.getString(R.string.files_progress, done, total))
            .setProgress(PROGRESS_MAX, TransferText.permille(transfer), transfer.state != TransferState.Running)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setContentIntent(openApp())
            .addAction(0, resources.getString(R.string.files_cancel), cancel(transfer.key))
            .build()
    }

    /** The notification for a transfer that ended, or null when there is nothing to tell. */
    private fun ended(transfer: Transfer): Notification? {
        val state = transfer.state
        // The user cancelled it here, and knows.
        if (state is TransferState.Cancelled && !state.byPeer) return null
        val resources = context.resources
        val incoming = transfer.key.direction == Direction.INCOMING
        val reason = TransferText.reason(transfer)?.resolve(resources)
        val names = TransferText.names(transfer).resolve(resources)
        val builder = NotificationCompat.Builder(context, CHANNEL_DONE)
            .setSmallIcon(if (incoming) R.drawable.ic_file_download else R.drawable.ic_file_upload)
            .setContentTitle(TransferText.status(transfer).resolve(resources))
            .setContentText(reason ?: names)
            .setAutoCancel(true)
            .setContentIntent(openApp())
        if (reason != null) builder.setStyle(NotificationCompat.BigTextStyle().bigText("$names\n$reason"))
        if (incoming && transfer.saved.isNotEmpty()) {
            val single = transfer.saved.singleOrNull()?.takeIf { it.uri.isNotEmpty() }
            if (single != null) {
                val open = activity(
                    Intent(Intent.ACTION_VIEW)
                        .setDataAndType(single.uri.toUri(), single.mimeType)
                        // Explicit: from Android 18, a URI in an intent no longer grants read access by itself.
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),
                )
                builder.setContentIntent(open).addAction(0, resources.getString(R.string.files_open), open)
            } else {
                builder.setContentIntent(activity(Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)))
            }
            builder.addAction(0, resources.getString(R.string.files_show_downloads), activity(Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)))
        }
        return builder.build()
    }

    private fun openApp(): PendingIntent? =
        context.packageManager.getLaunchIntentForPackage(context.packageName)?.let(::activity)

    private fun activity(intent: Intent): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun cancel(key: TransferKey): PendingIntent = PendingIntent.getBroadcast(
        context,
        0,
        TransferActionReceiver.cancelIntent(context, key),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun createChannels() {
        manager.createNotificationChannels(
            listOf(
                NotificationChannel(CHANNEL_PROGRESS, context.getString(R.string.files_channel_progress), NotificationManager.IMPORTANCE_LOW)
                    .apply { description = context.getString(R.string.files_channel_progress_description) },
                NotificationChannel(CHANNEL_DONE, context.getString(R.string.files_channel_done), NotificationManager.IMPORTANCE_DEFAULT)
                    .apply { description = context.getString(R.string.files_channel_done_description) },
            ),
        )
    }

    private fun notificationId(key: TransferKey): Int = key.hashCode()

    private companion object {
        /** Keeps these ids apart from the app's other notifications. */
        const val TAG = NotificationIds.FILES_TAG
        const val CHANNEL_PROGRESS = "file_transfers"
        const val CHANNEL_DONE = "file_transfers_done"
        const val PROGRESS_MAX = 1000
        const val UPDATE_MILLIS = 500L
    }
}
