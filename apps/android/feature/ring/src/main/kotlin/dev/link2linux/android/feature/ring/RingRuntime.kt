package dev.link2linux.android.feature.ring

import android.content.Context
import androidx.core.content.edit
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.ring.RingEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The process-wide ring feature: the setting, the engine, and the phone's
 * ringtone, vibration and notification. The session reaches the engine
 * through [engine] (see RingSessionHooks).
 */
@Singleton
class RingRuntime @Inject internal constructor(@ApplicationContext context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("ring", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + loggingUncaught("the ringing"))

    private val _enabled = MutableStateFlow(prefs.getBoolean(ENABLED, Feature.RING.isOnByDefault))

    /** Whether the user has the feature on: on until turned off, so it is on when the phone is lost. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    val engine = RingEngine(
        scope = scope,
        player = AndroidRingPlayer(app),
        vibrator = AndroidRingVibrator(app),
        notifier = RingNotifications(app),
        enabled = enabled,
    )

    fun setEnabled(on: Boolean) {
        prefs.edit { putBoolean(ENABLED, on) }
        _enabled.value = on
    }

    /** The user stopped the ringing on the phone. */
    fun stop() = engine.stopOnPhone()

    private companion object {
        const val ENABLED = "enabled"
    }
}
