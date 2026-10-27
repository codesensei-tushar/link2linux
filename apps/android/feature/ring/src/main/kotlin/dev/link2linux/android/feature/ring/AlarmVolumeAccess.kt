package dev.link2linux.android.feature.ring

import android.content.Context
import android.media.AudioManager
import android.util.Log
import androidx.core.content.edit
import dev.link2linux.android.core.ring.AlarmVolume
import dev.link2linux.android.core.ring.AlarmVolumeGuard
import dev.link2linux.android.core.ring.SavedVolume

private const val TAG = "link2linux"
private const val LEVEL = "before"

/** The guard over the device's alarm stream and the file that keeps the level from before ringing. */
internal fun alarmVolumeGuard(context: Context): AlarmVolumeGuard {
    val app = context.applicationContext
    return AlarmVolumeGuard(AlarmStreamVolume(app.getSystemService(AudioManager::class.java)), PreferencesSavedVolume(app))
}

private class AlarmStreamVolume(private val audio: AudioManager?) : AlarmVolume {
    override fun level() = audio?.getStreamVolume(AudioManager.STREAM_ALARM) ?: 0

    override fun max() = audio?.getStreamMaxVolume(AudioManager.STREAM_ALARM) ?: 0

    override fun muted() = audio?.isStreamMute(AudioManager.STREAM_ALARM) ?: true

    // Do Not Disturb may refuse the change; the guard checks that it took.
    override fun set(level: Int) {
        try {
            audio?.setStreamVolume(AudioManager.STREAM_ALARM, level, 0)
        } catch (e: SecurityException) {
            Log.w(TAG, "Could not set the alarm volume: ${e.javaClass.simpleName}")
        }
    }
}

/** The level from before ringing, in its own file and written with `commit` since the process may die right after. */
private class PreferencesSavedVolume(context: Context) : SavedVolume {
    private val prefs = context.getSharedPreferences("ring-volume", Context.MODE_PRIVATE)

    override fun load(): Int? = if (prefs.contains(LEVEL)) prefs.getInt(LEVEL, 0) else null

    override fun save(level: Int) = prefs.edit(commit = true) { putInt(LEVEL, level) }

    override fun clear() = prefs.edit(commit = true) { remove(LEVEL) }
}
