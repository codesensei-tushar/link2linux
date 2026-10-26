package dev.link2linux.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import dagger.hilt.android.AndroidEntryPoint
import dev.link2linux.android.core.designsystem.theme.L2lTheme
import dev.link2linux.android.service.ConnectionService
import dev.link2linux.android.ui.L2lApp

@AndroidEntryPoint
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            L2lTheme {
                L2lApp()
            }
        }
    }

    override fun onStart() {
        super.onStart()
        // The app is visible, which is when Android allows the foreground service to start.
        ConnectionService.start(this)
    }
}
