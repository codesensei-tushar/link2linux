package dev.link2linux.android.feature.clipboard

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.os.PersistableBundle
import dev.link2linux.android.core.clipboard.ClipText
import dev.link2linux.android.core.clipboard.ClipboardWriter
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * `ClipDescription.EXTRA_IS_SENSITIVE`: the source app marked the clip as
 * secret. Android 13 and later leave such clips out of the copy preview;
 * older versions ignore the extra. Spelled out because the constant needs
 * API 33.
 */
private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"

/**
 * Puts text on the system clipboard, which Android allows from the
 * background. Android 13 and later confirm the copy with a preview, which
 * hides text marked sensitive.
 */
internal class AndroidClipboardWriter(private val context: Context) : ClipboardWriter {
    override suspend fun write(clip: ClipText) = withContext(Dispatchers.Main) {
        val manager = checkNotNull(context.getSystemService(ClipboardManager::class.java)) { "No clipboard service" }
        val data = ClipData.newPlainText(context.getString(R.string.clipboard_clip_label), clip.text)
        if (clip.sensitive) data.description.extras = PersistableBundle().apply { putBoolean(EXTRA_IS_SENSITIVE, true) }
        manager.setPrimaryClip(data)
    }
}

/** What a clip holds, as far as it can be shared. */
internal sealed interface ClipContent {
    data class Text(val clip: ClipText) : ClipContent

    data object Empty : ClipContent

    /** Something other than text, such as an image or a file. */
    data object NotText : ClipContent
}

/**
 * The text of the clip's first item. Only text the clip holds counts:
 * an item that only points at something, a file or an image behind a URI
 * or an intent, is not read or turned into text.
 */
internal fun ClipData?.content(): ClipContent {
    if (this == null || itemCount == 0) return ClipContent.Empty
    val item = getItemAt(0)
    val text = item.text
    return when {
        text == null -> if (item.uri != null || item.intent != null) ClipContent.NotText else ClipContent.Empty
        text.isEmpty() -> ClipContent.Empty
        else -> ClipContent.Text(ClipText(text.toString(), sensitive = description.isSensitive()))
    }
}

/** The source app marked the clip as secret. */
internal fun ClipDescription.isSensitive(): Boolean = extras?.getBoolean(EXTRA_IS_SENSITIVE) == true

/**
 * The clip holds text, as far as its description tells. A list of URIs is
 * left out, since it points at files rather than holding text.
 */
internal fun ClipDescription.isText(): Boolean =
    hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN) || hasMimeType(ClipDescription.MIMETYPE_TEXT_HTML)
