package dev.link2linux.android.service

import android.content.Context
import android.content.Intent
import dev.link2linux.android.core.discovery.ConnectionSwitch
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Keeps the choice in the app's private preferences, on by default, and starts or stops the service with it. */
class PreferencesConnectionSwitch(context: Context) : ConnectionSwitch {

    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("connection", Context.MODE_PRIVATE)
    private val state = MutableStateFlow(prefs.getBoolean(KEY, true))

    override val enabled: StateFlow<Boolean> = state.asStateFlow()

    override fun setEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(KEY, enabled).apply()
        state.value = enabled
        if (enabled) ConnectionService.start(app) else app.stopService(Intent(app, ConnectionService::class.java))
    }

    private companion object {
        const val KEY = "enabled"
    }
}
