package dev.link2linux.android.feature.clipboard

import android.app.Activity
import android.app.PendingIntent
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Bundle

/**
 * Sends text to the connected computers when the user asks. It shows
 * nothing of its own and finishes at once; a toast tells how it went.
 *
 * Under its own name, which is not exported, it sends the clipboard: the
 * quick settings tile, the connection notification and the app start it.
 * Android 10 and later let an app read the clipboard only while it has
 * focus, so it reads once its window gains focus.
 *
 * Under the exported alias "Copy to computer", other apps start it from
 * the share sheet (`ACTION_SEND`) or the text selection menu
 * (`ACTION_PROCESS_TEXT`), and it sends only the text they hand over. It
 * never reads the clipboard there, so no other app can make it do so, and
 * it returns nothing, so the selected text stays as it is. A share that
 * carries a file (`EXTRA_STREAM`) is the file feature's to send, even
 * plain text, so it is not taken here.
 *
 * Unverified without a device: that the translucent window gains focus on
 * every Android version and launcher.
 */
class ClipboardSendActivity : Activity() {

    private var read = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val runtime = ClipboardRuntime.get(this)
        if (componentName.className != ClipboardSendActivity::class.java.name) {
            if (intent.hasExtra(Intent.EXTRA_STREAM)) {
                runtime.show(Message(R.string.clipboard_result_text_only))
            } else {
                runtime.share(givenText(intent))
            }
            finish()
            return
        }
        runtime.refusalBeforeReading()?.let {
            runtime.show(it)
            finish()
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || read || isFinishing) return
        read = true
        ClipboardRuntime.get(this).shareClipboard(getSystemService(ClipboardManager::class.java)?.primaryClip)
        finish()
    }

    private fun givenText(intent: Intent): String? = when (intent.action) {
        Intent.ACTION_SEND -> intent.getCharSequenceExtra(Intent.EXTRA_TEXT)
        Intent.ACTION_PROCESS_TEXT -> intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)
        else -> null
    }?.toString()

    companion object {
        /** Sends the clipboard. From outside an activity, add [Intent.FLAG_ACTIVITY_NEW_TASK]. */
        fun intent(context: Context): Intent = Intent(context, ClipboardSendActivity::class.java)

        /** Sends the clipboard, for a notification action or a tile. */
        fun pendingIntent(context: Context, requestCode: Int): PendingIntent = PendingIntent.getActivity(
            context,
            requestCode,
            intent(context).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }
}
