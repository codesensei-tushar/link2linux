package dev.link2linux.android

import android.content.ClipboardManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import dagger.hilt.android.AndroidEntryPoint
import dev.link2linux.android.core.data.ConnectionPolicy
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.designsystem.theme.L2lTheme
import dev.link2linux.android.core.discovery.ConnectionSwitch
import dev.link2linux.android.feature.clipboard.ClipboardRuntime
import dev.link2linux.android.service.CompanionAssociations
import dev.link2linux.android.service.ConnectionService
import dev.link2linux.android.ui.L2lApp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import javax.inject.Inject

@AndroidEntryPoint
class MainActivity : ComponentActivity() {

    @Inject lateinit var connectionSwitch: ConnectionSwitch
    @Inject lateinit var pairedDevices: PairedDeviceRepository
    @Inject lateinit var pairing: PairingController

    /** Shares a new copy while the window has focus, which is when Android lets the app read the clipboard. */
    private var clipboardShare: Job? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            L2lTheme {
                L2lApp()
            }
        }
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                // The app is visible, which is when Android allows the foreground service to start:
                // when the user turns the switch on, pairs the first desktop, or opens the app.
                launch {
                    combine(pairedDevices.pairedDevices, pairing.state, connectionSwitch.enabled, ConnectionPolicy::shouldRun)
                        .distinctUntilChanged()
                        .collect { run -> if (run) ConnectionService.start(this@MainActivity) }
                }
                // Unpairing happens here, possibly while the service is off. The association calls are binder calls.
                launch(Dispatchers.Default) { CompanionAssociations(this@MainActivity).follow(pairedDevices.pairedDevices) }
            }
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        clipboardShare?.cancel()
        clipboardShare = if (hasFocus) {
            lifecycleScope.launch {
                ClipboardRuntime.get(this@MainActivity).shareWhileFocused(getSystemService(ClipboardManager::class.java))
            }
        } else {
            null
        }
    }
}
