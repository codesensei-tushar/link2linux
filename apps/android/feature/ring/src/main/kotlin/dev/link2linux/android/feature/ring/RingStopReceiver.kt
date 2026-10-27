package dev.link2linux.android.feature.ring

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.android.EntryPointAccessors
import dagger.hilt.components.SingletonComponent

/**
 * Stop on the ringing notification. Not exported: only the app's own
 * PendingIntents reach it.
 */
class RingStopReceiver : BroadcastReceiver() {

    @EntryPoint
    @InstallIn(SingletonComponent::class)
    internal interface Ring {
        fun ringRuntime(): RingRuntime
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_STOP) return
        EntryPointAccessors.fromApplication(context.applicationContext, Ring::class.java).ringRuntime().stop()
    }

    internal companion object {
        private const val ACTION_STOP = "dev.link2linux.android.ring.STOP"

        fun stopIntent(context: Context): Intent = Intent(context, RingStopReceiver::class.java).setAction(ACTION_STOP)
    }
}
