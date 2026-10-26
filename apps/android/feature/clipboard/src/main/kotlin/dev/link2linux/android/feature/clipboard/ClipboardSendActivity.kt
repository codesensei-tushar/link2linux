package dev.link2linux.android.feature.clipboard

import android.app.Activity
import android.app.PendingIntent
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper

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
 * every Android version and launcher. Where it does not, it gives up after
 * [FOCUS_TIMEOUT_MILLIS] rather than sit unseen over the screen, where it
 * would take every touch.
 */
class ClipboardSendActivity : Activity() {

    private val handler = Handler(Looper.getMainLooper())
    private var read = false

    private val giveUp = Runnable {
        if (!read && !isFinishing) {
            read = true
            ClipboardRuntime.get(this).show(Message(R.string.clipboard_result_no_focus))
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val runtime = ClipboardRuntime.get(this)
        if (componentName.className != ClipboardSendActivity::class.java.name) {
            val handed = handedOver(hasFile = { intent.hasExtra(Intent.EXTRA_STREAM) }, text = { givenText(intent) })
            when (handed) {
                Handed.File -> runtime.show(Message(R.string.clipboard_result_text_only))
                is Handed.Text -> runtime.share(handed.text)
            }
            finish()
            return
        }
        val refusal = runtime.refusalBeforeReading()
        if (refusal != null) {
            runtime.show(refusal)
            finish()
        } else {
            handler.postDelayed(giveUp, FOCUS_TIMEOUT_MILLIS)
        }
    }

    override fun onDestroy() {
        handler.removeCallbacks(giveUp)
        super.onDestroy()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || read || isFinishing) return
        read = true
        ClipboardRuntime.get(this).shareClipboard(getSystemService(ClipboardManager::class.java)?.primaryClip)
        finish()
    }

    private fun givenText(intent: Intent): CharSequence? = when (intent.action) {
        Intent.ACTION_SEND -> intent.getCharSequenceExtra(Intent.EXTRA_TEXT)
        Intent.ACTION_PROCESS_TEXT -> intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)
        else -> null
    }

    companion object {
        /** How long the window may take to gain focus; it normally takes well under a second. */
        private const val FOCUS_TIMEOUT_MILLIS = 2_000L

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

/** What another app handed over through "Copy to computer". */
internal sealed interface Handed {
    data class Text(val text: String?) : Handed

    /** A file, which is the file feature's to send, even when it holds plain text. */
    data object File : Handed
}

/**
 * Reads what another app handed over, [hasFile] and [text] reading the
 * intent's extras. Any app can start the alias with any extras, and reading
 * one whose class this app cannot load throws; that counts as nothing to
 * share rather than ending the process, and every session with it.
 */
internal fun handedOver(hasFile: () -> Boolean, text: () -> CharSequence?): Handed = try {
    if (hasFile()) Handed.File else Handed.Text(text()?.toString())
} catch (_: RuntimeException) {
    Handed.Text(null)
}
