package dev.link2linux.android.feature.ring

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaPlayer
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import dev.link2linux.android.core.ring.AlarmVolume
import dev.link2linux.android.core.ring.AlarmVolumeGuard
import dev.link2linux.android.core.ring.RingPlayer
import dev.link2linux.android.core.ring.SavedVolume

/**
 * Plays the ringtone on the alarm stream, which Android plays also when the
 * ringer is set to vibrate or silent, at the stream's full volume. The
 * volume goes back to what it was when ringing stops, or at the next start
 * of the app if it died meanwhile (see [AlarmVolumeGuard]).
 *
 * The phone does not ring while the audio is in any mode but the normal
 * one, as during a call or while a call rings, or when it cannot take the
 * audio focus. Once ringing, it stops when it loses the focus to an app
 * that does not just lower it, such as the phone app with an incoming
 * call, or when a call starts.
 *
 * The engine calls [start] and [stop] one at a time, so the state needs no
 * lock of its own.
 */
internal class AndroidRingPlayer(context: Context) : RingPlayer {
    private val app = context.applicationContext
    private val audio = app.getSystemService(AudioManager::class.java)
    private val main = Handler(Looper.getMainLooper())

    private var media: MediaPlayer? = null
    private var focus: AudioFocusRequest? = null
    private val volume = AlarmVolumeGuard(
        object : AlarmVolume {
            override fun level() = audio?.getStreamVolume(AudioManager.STREAM_ALARM) ?: 0

            override fun max() = audio?.getStreamMaxVolume(AudioManager.STREAM_ALARM) ?: 0

            override fun set(level: Int) {
                setAlarmVolume(level)
            }
        },
        PreferencesSavedVolume(app),
    )
    // An AudioManager.OnModeChangedListener from Android 12 on; typed so older versions never load the type.
    private var modeListener: Any? = null

    init {
        // Nothing rings at the start of the process, so a raised volume left behind is a crash's.
        volume.repair()
    }

    override fun start(onInterrupted: () -> Unit): Boolean {
        val audio = audio ?: return false
        if (audio.mode != AudioManager.MODE_NORMAL) return false
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(ALARM)
            .setOnAudioFocusChangeListener(
                { change ->
                    // Lowering the volume for a moment is not a reason to stop: the phone keeps ringing.
                    if (change == AudioManager.AUDIOFOCUS_LOSS || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) onInterrupted()
                },
                main,
            )
            .build()
        if (audio.requestAudioFocus(request) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) return false
        focus = request
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val listener = AudioManager.OnModeChangedListener { mode -> if (mode != AudioManager.MODE_NORMAL) onInterrupted() }
            audio.addOnModeChangedListener(ContextCompat.getMainExecutor(app), listener)
            modeListener = listener
        }
        volume.raise()
        // Without any ringtone that plays, the phone still vibrates and shows the notification.
        media = TONES.firstNotNullOfOrNull(::play)
        return true
    }

    override fun stop() {
        media?.run {
            runCatching { stop() }
            release()
        }
        media = null
        volume.restore()
        val audio = audio ?: return
        focus?.let(audio::abandonAudioFocusRequest)
        focus = null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            (modeListener as? AudioManager.OnModeChangedListener)?.let(audio::removeOnModeChangedListener)
        }
        modeListener = null
    }

    /** Plays [tone] in a loop, or returns null when it cannot, as for a ringtone set to none. */
    private fun play(tone: Uri): MediaPlayer? {
        val player = MediaPlayer()
        return runCatching {
            player.setAudioAttributes(ALARM)
            player.setDataSource(app, tone)
            player.isLooping = true
            player.prepare()
            player.start()
            player
        }.onFailure {
            player.release()
            Log.w(TAG, "Could not play a ringing tone: ${it.javaClass.simpleName}")
        }.getOrNull()
    }

    // Do Not Disturb may refuse the change; the phone then rings as loud as the user left it.
    private fun setAlarmVolume(volume: Int) {
        try {
            audio?.setStreamVolume(AudioManager.STREAM_ALARM, volume, 0)
        } catch (e: SecurityException) {
            Log.w(TAG, "Could not set the alarm volume: ${e.javaClass.simpleName}")
        }
    }

    /** The level from before ringing, in its own file and written with `commit` since the process may die right after. */
    private class PreferencesSavedVolume(context: Context) : SavedVolume {
        private val prefs = context.getSharedPreferences("ring-volume", Context.MODE_PRIVATE)

        override fun load(): Int? = if (prefs.contains(LEVEL)) prefs.getInt(LEVEL, 0) else null

        override fun save(level: Int) = prefs.edit(commit = true) { putInt(LEVEL, level) }

        override fun clear() = prefs.edit(commit = true) { remove(LEVEL) }
    }

    private companion object {
        const val LEVEL = "before"
        const val TAG = "link2linux"

        val ALARM: AudioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_ALARM)
            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
            .build()

        /**
         * The user's ringtone first, then the alarm and notification sounds.
         * Any app may play these: Android serves a copy of each through the
         * settings provider, so no storage permission is needed.
         */
        val TONES: List<Uri> = listOf(
            Settings.System.DEFAULT_RINGTONE_URI,
            Settings.System.DEFAULT_ALARM_ALERT_URI,
            Settings.System.DEFAULT_NOTIFICATION_URI,
        )
    }
}
