package dev.link2linux.android.feature.clipboard

import android.content.ClipData
import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.util.Log
import android.widget.Toast
import androidx.core.content.edit
import dev.link2linux.android.core.clipboard.ClipText
import dev.link2linux.android.core.clipboard.ClipboardEngine
import dev.link2linux.android.core.clipboard.ShareResult
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The process-wide clipboard feature: the setting, the engine, and the glue
 * to the system clipboard. The session reaches the engine through [engine]
 * (see ClipboardSessionHooks). The ways to share call [share] or
 * [shareClipboard], and a toast tells the user how it went.
 */
class ClipboardRuntime private constructor(context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("clipboard", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val _enabled = MutableStateFlow(prefs.getBoolean(ENABLED, false))

    /** The user turned the feature on. Off until then, like every feature. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    val engine = ClipboardEngine(
        scope = scope,
        writer = AndroidClipboardWriter(app),
        enabled = enabled,
        // The exception's type only: a message could quote the text.
        onError = { Log.w(TAG, "Could not set the clipboard: ${it.javaClass.simpleName}") },
    )

    init {
        // The share sheet and selection entries follow the setting, even after the app's data was cleared.
        scope.launch { showShareTargets(enabled.value) }
    }

    fun setEnabled(on: Boolean) {
        prefs.edit { putBoolean(ENABLED, on) }
        _enabled.value = on
        scope.launch { showShareTargets(on) }
    }

    /** Shares text the user handed over, from the share sheet or a text selection. */
    fun share(text: String?) {
        scope.launch { show(engine.share(ClipText(text.orEmpty()))) }
    }

    /** Shares what the clipboard holds. Read [clip] while the app has focus, which Android requires. */
    fun shareClipboard(clip: ClipData?) {
        val content = clip.content()
        scope.launch {
            when (content) {
                is ClipContent.Text -> show(engine.share(content.clip))
                ClipContent.Empty -> show(Message(R.string.clipboard_result_empty))
                ClipContent.NotText -> show(Message(R.string.clipboard_result_text_only))
            }
        }
    }

    /**
     * Why sharing the clipboard would fail whatever it holds, or null when it
     * might work. Asked before reading the clipboard, so Android does not
     * tell the user about a read that was for nothing.
     */
    internal fun refusalBeforeReading(): Message? = when {
        !enabled.value -> messageFor(ShareResult.Disabled)
        engine.connected.value.isEmpty() -> messageFor(ShareResult.NoDesktop)
        else -> null
    }

    internal fun show(message: Message) {
        scope.launch { showNow(message) }
    }

    private suspend fun show(result: ShareResult) {
        messageFor(result)?.let { showNow(it) }
    }

    // Plain text toasts may be shown from the background.
    private suspend fun showNow(message: Message) = withContext(Dispatchers.Main) {
        Toast.makeText(app, app.getString(message.id, *message.args.toTypedArray()), Toast.LENGTH_SHORT).show()
    }

    /** Lists "Copy to computer" in the share sheet and the text selection menu only while the feature is on. */
    private fun showShareTargets(on: Boolean) {
        val component = ComponentName(app.packageName, COPY_TO_COMPUTER)
        val wanted = if (on) PackageManager.COMPONENT_ENABLED_STATE_ENABLED else PackageManager.COMPONENT_ENABLED_STATE_DEFAULT
        val pm = app.packageManager
        if (pm.getComponentEnabledSetting(component) != wanted) {
            pm.setComponentEnabledSetting(component, wanted, PackageManager.DONT_KILL_APP)
        }
    }

    companion object {
        private const val TAG = "link2linux"
        private const val ENABLED = "enabled"

        /** The exported alias of [ClipboardSendActivity] that other apps share text to; off in the manifest. */
        internal const val COPY_TO_COMPUTER = "dev.link2linux.android.feature.clipboard.CopyToComputer"

        @Volatile
        private var instance: ClipboardRuntime? = null

        fun get(context: Context): ClipboardRuntime =
            instance ?: synchronized(this) {
                instance ?: ClipboardRuntime(context).also { instance = it }
            }
    }
}
