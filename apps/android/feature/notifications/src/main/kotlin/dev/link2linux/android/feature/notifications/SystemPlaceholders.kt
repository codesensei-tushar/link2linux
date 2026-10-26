package dev.link2linux.android.feature.notifications

import android.annotation.SuppressLint
import android.content.res.Resources

/**
 * The text Android puts in place of withheld notification content, in the
 * phone's language, read from the system's own resources. Best effort: the
 * resource may be missing or named differently on a given Android version or
 * vendor build, in which case only the English default is matched.
 *
 * The wording for locked apps (Android 17 and later) is not known, so
 * [dev.link2linux.android.core.notifications.RedactionDetector] gets no
 * placeholder for that case yet; its `appLocked` set is the place to add one.
 */
internal object SystemPlaceholders {
    @SuppressLint("DiscouragedApi")
    fun bySystem(): Set<String> = try {
        val res = Resources.getSystem()
        val id = res.getIdentifier("redacted_notification_message", "string", "android")
        if (id == 0) emptySet() else setOf(res.getString(id))
    } catch (_: RuntimeException) {
        emptySet()
    }
}
