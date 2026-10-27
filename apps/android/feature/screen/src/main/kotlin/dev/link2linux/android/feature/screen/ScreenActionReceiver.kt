package dev.link2linux.android.feature.screen

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Don't share on the question's notification, and Stop sharing on the
 * sharing one. Not exported: only the app's own PendingIntents reach it.
 */
class ScreenActionReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val runtime = ScreenRuntime.from(context)
        when (intent.action) {
            ACTION_DECLINE -> runtime.declined(intent.getLongExtra(EXTRA_ASK_ID, 0))
            ACTION_STOP -> runtime.stop()
        }
    }

    internal companion object {
        private const val ACTION_DECLINE = "dev.link2linux.android.screen.DECLINE"
        private const val ACTION_STOP = "dev.link2linux.android.screen.STOP"
        private const val EXTRA_ASK_ID = "dev.link2linux.android.screen.ASK_ID"

        fun declineIntent(context: Context, askId: Long): Intent =
            Intent(context, ScreenActionReceiver::class.java).setAction(ACTION_DECLINE).putExtra(EXTRA_ASK_ID, askId)

        fun stopIntent(context: Context): Intent = Intent(context, ScreenActionReceiver::class.java).setAction(ACTION_STOP)
    }
}
