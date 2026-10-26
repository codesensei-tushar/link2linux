package dev.link2linux.android.service

import android.content.Context
import android.content.Intent
import dev.link2linux.android.core.discovery.ConnectionSwitch
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Keeps the choice in the app's private preferences, on by default, and
 * stops the service when it is turned off. Turning it on is always done in
 * the app, which starts the service once there is something to connect.
 */
class PreferencesConnectionSwitch(context: Context) : ConnectionSwitch {

    private val app = context.applicationContext
    private val prefs = preferences(app)
    private val state = MutableStateFlow(prefs.getBoolean(KEY, true))

    override val enabled: StateFlow<Boolean> = state.asStateFlow()

    override fun setEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(KEY, enabled).apply()
        state.value = enabled
        if (!enabled) app.stopService(Intent(app, ConnectionService::class.java))
    }

    companion object {
        private const val KEY = "enabled"

        private fun preferences(context: Context) = context.getSharedPreferences("connection", Context.MODE_PRIVATE)

        /** The user's choice, for a component Android starts outside the dependency graph. */
        fun isOn(context: Context): Boolean = preferences(context.applicationContext).getBoolean(KEY, true)
    }
}
