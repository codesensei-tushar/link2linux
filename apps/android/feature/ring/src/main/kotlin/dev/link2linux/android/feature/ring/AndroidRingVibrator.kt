package dev.link2linux.android.feature.ring

import android.content.Context
import android.media.AudioAttributes
import android.os.Build
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import dev.link2linux.android.core.ring.RingVibrator

/** Vibrates as an alarm does, which Android allows also in silent mode, until stopped. */
internal class AndroidRingVibrator(context: Context) : RingVibrator {
    private val vibrator: Vibrator? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        context.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        context.getSystemService(Vibrator::class.java)
    }

    override fun start() {
        val vibrator = vibrator?.takeIf { it.hasVibrator() } ?: return
        val effect = VibrationEffect.createWaveform(PATTERN, 0)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            vibrator.vibrate(effect, VibrationAttributes.createForUsage(VibrationAttributes.USAGE_ALARM))
        } else {
            @Suppress("DEPRECATION")
            vibrator.vibrate(effect, AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ALARM).build())
        }
    }

    override fun stop() {
        vibrator?.cancel()
    }

    private companion object {
        /** Off, on, off, in milliseconds, repeated from the start. */
        val PATTERN = longArrayOf(0, 800, 600)
    }
}
