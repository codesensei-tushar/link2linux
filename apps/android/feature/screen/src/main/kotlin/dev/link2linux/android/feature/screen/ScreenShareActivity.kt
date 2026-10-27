package dev.link2linux.android.feature.screen

import android.app.Activity
import android.app.AlertDialog
import android.app.NotificationManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import dev.link2linux.android.core.common.NotificationIds
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/**
 * Asks whether to share the screen with a computer. Opened by the app
 * while it is in front, it first names the computer in a dialog of its
 * own; opened from the question's notification, which named it already, it
 * goes straight to Android's question. Android's answer goes to
 * [ScreenRuntime]. It leaves when the question is taken back, for example
 * because the minute to answer is over.
 *
 * On Android 14 and later it asks for the whole screen: the computer
 * controls the phone by the screen's coordinates, which a single app's
 * window would not match.
 */
class ScreenShareActivity : Activity() {

    private val scope = MainScope()
    private var askId = 0L
    private var launched = false
    private var dialog: AlertDialog? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val runtime = ScreenRuntime.from(this)
        askId = intent.getLongExtra(EXTRA_ASK_ID, 0)
        launched = savedInstanceState?.getBoolean(STATE_LAUNCHED) ?: false
        // The question is set before this starts, and taken back once answered, timed out or no longer wanted.
        if (runtime.asker.current.value?.askId != askId) {
            finish()
            return
        }
        // Answered here, so the notification has nothing more to ask.
        getSystemService(NotificationManager::class.java)?.cancel(NotificationIds.SCREEN_ASK)
        scope.launch {
            runtime.asker.current.first { it?.askId != askId }
            finishActivity(REQUEST_CONSENT)
            finish()
        }
        // Recreated while Android's question is open: its answer comes to onActivityResult.
        if (launched) return
        val desktopName = intent.getStringExtra(EXTRA_DESKTOP_NAME).orEmpty()
        if (intent.getBooleanExtra(EXTRA_CONFIRM_FIRST, false)) confirm(desktopName) else askAndroid()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(STATE_LAUNCHED, launched)
    }

    override fun onDestroy() {
        scope.cancel()
        dialog?.dismiss()
        super.onDestroy()
    }

    private fun confirm(desktopName: String) {
        dialog = AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert)
            .setTitle(getString(R.string.screen_ask_title, desktopName))
            .setMessage(getString(R.string.screen_ask_dialog_body, desktopName))
            .setPositiveButton(R.string.screen_ask_dialog_continue) { _, _ -> askAndroid() }
            .setNegativeButton(R.string.screen_ask_decline) { _, _ -> decline() }
            .setOnCancelListener { decline() }
            .show()
    }

    private fun askAndroid() {
        val manager = getSystemService(MediaProjectionManager::class.java) ?: return decline()
        val ask = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            manager.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            manager.createScreenCaptureIntent()
        }
        launched = true
        try {
            startActivityForResult(ask, REQUEST_CONSENT)
        } catch (_: ActivityNotFoundException) {
            decline()
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_CONSENT) return
        val runtime = ScreenRuntime.from(this)
        if (resultCode == RESULT_OK && data != null) runtime.consented(askId, resultCode, data) else runtime.declined(askId)
        finish()
    }

    private fun decline() {
        ScreenRuntime.from(this).declined(askId)
        finish()
    }

    internal companion object {
        private const val REQUEST_CONSENT = 1
        private const val EXTRA_ASK_ID = "dev.link2linux.android.screen.ASK_ID"
        private const val EXTRA_DESKTOP_NAME = "dev.link2linux.android.screen.DESKTOP_NAME"
        private const val EXTRA_CONFIRM_FIRST = "dev.link2linux.android.screen.CONFIRM_FIRST"
        private const val STATE_LAUNCHED = "launched"

        fun intent(context: Context, question: ScreenAsk.Question, confirmFirst: Boolean): Intent =
            Intent(context, ScreenShareActivity::class.java)
                .putExtra(EXTRA_ASK_ID, question.askId)
                .putExtra(EXTRA_DESKTOP_NAME, question.desktopName)
                .putExtra(EXTRA_CONFIRM_FIRST, confirmFirst)
    }
}
