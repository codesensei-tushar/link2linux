package dev.link2linux.android.feature.ring

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Undoes a raised alarm volume that a power-off while ringing left behind.
 * It starts nothing else: the volume file and the audio service are all it touches.
 */
class AlarmVolumeBootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        alarmVolumeGuard(context).repair()
    }
}
