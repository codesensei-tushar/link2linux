package dev.link2linux.android.feature.status

import android.content.Context
import androidx.core.content.edit
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.status.StatusEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The process-wide phone status feature: the setting and the engine. The
 * session reaches the engine through [engine] (see StatusSessionHooks).
 */
@Singleton
class StatusRuntime @Inject internal constructor(@ApplicationContext context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("status", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + loggingUncaught("the phone status"))

    private val _enabled = MutableStateFlow(prefs.getBoolean(ENABLED, Feature.STATUS.isOnByDefault))

    /** Whether the user has the feature on: on until turned off. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    val engine = StatusEngine(scope, AndroidStatusSource(app), enabled)

    fun setEnabled(on: Boolean) {
        prefs.edit { putBoolean(ENABLED, on) }
        _enabled.value = on
    }

    private companion object {
        const val ENABLED = "enabled"
    }
}
