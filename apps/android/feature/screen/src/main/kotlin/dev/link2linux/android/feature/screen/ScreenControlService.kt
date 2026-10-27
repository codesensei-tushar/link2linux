package dev.link2linux.android.feature.screen

import android.accessibilityservice.AccessibilityService
import android.content.Intent
import android.view.accessibility.AccessibilityEvent

/**
 * The accessibility service through which the desktop controls the phone,
 * once the user turned it on in Android's settings. It does nothing of its
 * own: while it runs, [AndroidScreenController] carries out the desktop's
 * input through it, and only while the screen is shared with that desktop
 * and the user allows control in the app.
 */
class ScreenControlService : AccessibilityService() {

    override fun onServiceConnected() {
        super.onServiceConnected()
        ScreenRuntime.from(this).controlConnected(this)
    }

    override fun onUnbind(intent: Intent?): Boolean {
        ScreenRuntime.from(this).controlDisconnected(this)
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        ScreenRuntime.from(this).controlDisconnected(this)
        super.onDestroy()
    }

    // The focus and window events keep Android's view of the focused text field current; nothing reads them.
    override fun onAccessibilityEvent(event: AccessibilityEvent?) = Unit

    override fun onInterrupt() = Unit
}
