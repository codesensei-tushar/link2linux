package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.NotificationContent

/**
 * Spots content Android withheld from this app. Android gives normal apps no
 * flag for it, so detection compares the text fields with the placeholders the
 * system puts in their place. It is best effort: a placeholder in a language
 * or wording that is not listed passes as ordinary content.
 *
 * @param bySystem placeholders for notifications that look like they hold a
 *   one-time code (Android 15 and later).
 * @param appLocked placeholders for apps the user locked (Android 17 and
 *   later). Empty by default, since the wording is not known: the Android
 *   adapter adds what it can read from the system's own resources.
 */
class RedactionDetector(
    private val bySystem: Set<String> = DEFAULT_BY_SYSTEM,
    private val appLocked: Set<String> = emptySet(),
) {
    private val bySystemNormalized = bySystem.map(::normalize).toSet()
    private val appLockedNormalized = appLocked.map(::normalize).toSet()

    /** FULL, REDACTED_BY_SYSTEM or REDACTED_APP_LOCKED. */
    fun detect(snapshot: NotificationSnapshot): NotificationContent {
        val fields = buildList {
            add(snapshot.title)
            add(snapshot.bigTitle)
            add(snapshot.text)
            add(snapshot.bigText)
            add(snapshot.subText)
            snapshot.conversation?.messages?.forEach { add(it.text) }
        }.map(::normalize).filter { it.isNotEmpty() }
        return when {
            fields.any { it in appLockedNormalized } -> NotificationContent.NOTIFICATION_CONTENT_REDACTED_APP_LOCKED
            fields.any { it in bySystemNormalized } -> NotificationContent.NOTIFICATION_CONTENT_REDACTED_BY_SYSTEM
            else -> NotificationContent.NOTIFICATION_CONTENT_FULL
        }
    }

    /** True when [text] is one of the known placeholders. */
    fun isPlaceholder(text: String): Boolean {
        val n = normalize(text)
        return n in bySystemNormalized || n in appLockedNormalized
    }

    companion object {
        /** Android 15's English placeholder text. Unverified on a device. */
        val DEFAULT_BY_SYSTEM: Set<String> = setOf("Sensitive notification content hidden")

        private fun normalize(text: String): String =
            text.trim().trimEnd('.', '…').replace(Regex("\\s+"), " ").lowercase()
    }
}
