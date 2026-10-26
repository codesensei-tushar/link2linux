package dev.link2linux.android.core.notifications

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ConversationMessage
import dev.link2linux.protocol.v1.NotificationAction
import dev.link2linux.protocol.v1.NotificationActionKind
import dev.link2linux.protocol.v1.NotificationCategory
import dev.link2linux.protocol.v1.NotificationContent
import dev.link2linux.protocol.v1.NotificationImportance
import dev.link2linux.protocol.v1.NotificationVisibility
import dev.link2linux.protocol.v1.PhoneNotification
import dev.link2linux.protocol.v1.conversationMessage
import dev.link2linux.protocol.v1.notificationAction
import dev.link2linux.protocol.v1.notificationProgress
import dev.link2linux.protocol.v1.phoneNotification

/** A notification ready to share, with what the engine needs to act on it later. */
class MappedNotification(
    val platformKey: String,
    /** The wire message with revision 0; the collection assigns the revision. */
    val notification: PhoneNotification,
    /** The actions that can run, by `NotificationAction.id`. */
    val actions: Map<String, ActionSnapshot>,
    val alertWhenNew: Boolean,
    val alertOnUpdate: Boolean,
    val imageHashes: Set<ByteString>,
    /** An image is still rendering: mapping again once it is done adds it. */
    val imagesPending: Boolean = false,
)

/**
 * Turns a [NotificationSnapshot] into the protocol's `PhoneNotification`. Pure
 * apart from the image store, so it is covered by JVM tests.
 */
class NotificationMapper(
    private val images: ImageStore,
    private val redaction: RedactionDetector = RedactionDetector(),
    private val oneTimeCodes: OneTimeCodeDetector = OneTimeCodeDetector(),
) {
    /**
     * @param contentHidden the user chose not to share this app's content.
     * @param hideOneTimeCodes the user keeps one-time codes on the phone.
     */
    fun map(s: NotificationSnapshot, contentHidden: Boolean = false, hideOneTimeCodes: Boolean = true): MappedNotification {
        val content = when {
            contentHidden -> NotificationContent.NOTIFICATION_CONTENT_HIDDEN_BY_USER
            // Before Android's own redaction, so a code in a field Android left alone is caught too.
            hideOneTimeCodes && oneTimeCodes.detect(s) -> NotificationContent.NOTIFICATION_CONTENT_ONE_TIME_CODE
            else -> redaction.detect(s)
        }
        val full = content == NotificationContent.NOTIFICATION_CONTENT_FULL
        // Hidden by the user or holding a code: only the app, the time and the category.
        // Redacted by Android: whatever text it left, but nothing that could hold the withheld part.
        val withheld = content == NotificationContent.NOTIFICATION_CONTENT_HIDDEN_BY_USER ||
            content == NotificationContent.NOTIFICATION_CONTENT_ONE_TIME_CODE

        val builtActions = if (withheld) emptyList() else buildActions(s, allowReply = full)
        val appIcon = images.intern(s.appIcon, ImageKind.APP_ICON)
        val largeIcon = if (full) images.intern(s.largeIcon, ImageKind.LARGE_ICON) else null
        val picture = if (full) images.intern(s.picture, ImageKind.PICTURE) else null
        val messages = if (full) s.conversation?.messages.orEmpty() else emptyList()

        fun clean(value: String, max: Int): String =
            if (withheld || redaction.isPlaceholder(value)) "" else WireText.truncate(value, max)

        val title = clean(s.bigTitle.ifBlank { s.title }, Limits.SHORT_TEXT)
        var body = clean(s.bigText.ifBlank { s.text }, Limits.LONG_TEXT)
        if (body.isEmpty() && messages.isNotEmpty()) body = WireText.truncate(messages.last().text, Limits.LONG_TEXT)
        val group = s.conversation?.isGroup == true
        val importance = importanceOf(s.importance)

        val notification = phoneNotification {
            key = WireText.identifier(s.key)
            appId = s.packageName
            appName = WireText.truncate(s.appName, Limits.APP_NAME)
            appIcon?.let { this.appIcon = it }
            whenUnixMs = if (s.eventTimeMillis > 0) s.eventTimeMillis else s.postTimeMillis
            if (!withheld) {
                this.title = title
                text = body
                subText = clean(s.subText, Limits.SHORT_TEXT)
            }
            category = categoryOf(s)
            this.importance = importance
            ongoing = s.isOngoing
            s.groupKey?.let { this.group = WireText.identifier(it) }
            actions += builtActions.map { it.first }
            this.messages += messages.takeLast(Limits.MESSAGES).map(::message)
            if (group && full) conversationTitle = clean(s.conversation.title, Limits.SHORT_TEXT)
            s.progress?.takeIf { !withheld }?.let { p ->
                progress = notificationProgress {
                    current = p.current.coerceAtLeast(0)
                    max = p.max.coerceAtLeast(0)
                    indeterminate = p.indeterminate
                }
            }
            largeIcon?.let { this.largeIcon = it }
            picture?.let { this.picture = it }
            this.content = content
            visibility = visibilityOf(s.visibility)
            dismissible = s.isClearable && !s.isOngoing
            isGroupConversation = group
        }

        val loud = importance == NotificationImportance.NOTIFICATION_IMPORTANCE_DEFAULT ||
            importance == NotificationImportance.NOTIFICATION_IMPORTANCE_HIGH
        val groupSilenced = if (s.isGroupSummary) {
            s.groupAlertBehavior == AndroidValues.GROUP_ALERT_CHILDREN
        } else {
            s.groupKey != null && s.groupAlertBehavior == AndroidValues.GROUP_ALERT_SUMMARY
        }
        val alerts = loud && !s.interruptionSuppressed && !groupSilenced

        return MappedNotification(
            platformKey = s.key,
            notification = notification,
            actions = builtActions.associate { (wire, snap) -> wire.id to snap },
            alertWhenNew = alerts,
            alertOnUpdate = alerts && !s.onlyAlertOnce,
            imageHashes = listOfNotNull(appIcon, largeIcon, picture).mapTo(mutableSetOf()) { it.sha256 },
            imagesPending = images.isPending(s.appIcon, ImageKind.APP_ICON) ||
                (full && (images.isPending(s.largeIcon, ImageKind.LARGE_ICON) || images.isPending(s.picture, ImageKind.PICTURE))),
        )
    }

    private fun message(m: MessageSnapshot): ConversationMessage = conversationMessage {
        sender = if (m.outgoing) "" else WireText.truncate(m.sender, Limits.SENDER)
        text = WireText.truncate(m.text, Limits.LONG_TEXT)
        sentUnixMs = m.timeMillis
        outgoing = m.outgoing
    }

    private fun buildActions(
        s: NotificationSnapshot,
        allowReply: Boolean,
    ): List<Pair<NotificationAction, ActionSnapshot>> {
        val used = HashMap<String, Int>()
        val result = ArrayList<Pair<NotificationAction, ActionSnapshot>>()
        // Call actions first, so the 8-action limit never drops them.
        val ordered = s.actions.sortedBy { if (it.callRole != null) 0 else 1 }
        for (a in ordered) {
            if (result.size >= Limits.ACTIONS) break
            val takesReply = a.reply != null
            if (takesReply && !allowReply) continue
            val base = idOf(a)
            val n = used.merge(base, 1, Int::plus)!!
            val id = if (n == 1) base else "$base:$n"
            val wire = notificationAction {
                this.id = id
                label = WireText.truncate(a.label.ifBlank { defaultLabel(a) }, Limits.SHORT_TEXT)
                kind = kindOf(a)
                requiresUnlock = a.requiresUnlock
                a.reply?.let { r ->
                    acceptsReply = r.allowFreeForm
                    val choices = (r.choices + if (r.allowGeneratedReplies) s.suggestedReplies else emptyList())
                        .filter { it.isNotBlank() }
                        .distinct()
                        .take(Limits.REPLY_CHOICES)
                    replyChoices += choices.map { WireText.truncate(it, Limits.REPLY_CHOICE) }
                }
            }
            result += wire to a
        }
        return result
    }

    private fun defaultLabel(a: ActionSnapshot): String = when (a.callRole) {
        CallRole.ANSWER -> "Answer"
        CallRole.DECLINE -> "Decline"
        CallRole.HANG_UP -> "Hang up"
        null -> ""
    }

    private fun idOf(a: ActionSnapshot): String = when (a.callRole) {
        CallRole.ANSWER -> "call:answer"
        CallRole.DECLINE -> "call:decline"
        CallRole.HANG_UP -> "call:hangup"
        null -> "a:" + WireText.shortHash(a.label, a.semanticAction.toString(), a.replyKey.orEmpty())
    }

    private fun kindOf(a: ActionSnapshot): NotificationActionKind = when (a.callRole) {
        CallRole.ANSWER -> NotificationActionKind.NOTIFICATION_ACTION_KIND_ANSWER_CALL
        CallRole.DECLINE -> NotificationActionKind.NOTIFICATION_ACTION_KIND_DECLINE_CALL
        CallRole.HANG_UP -> NotificationActionKind.NOTIFICATION_ACTION_KIND_HANG_UP
        null -> when {
            a.semanticAction == AndroidValues.SEMANTIC_REPLY || a.reply != null ->
                NotificationActionKind.NOTIFICATION_ACTION_KIND_REPLY
            a.semanticAction == AndroidValues.SEMANTIC_MARK_AS_READ -> NotificationActionKind.NOTIFICATION_ACTION_KIND_MARK_READ
            a.semanticAction == AndroidValues.SEMANTIC_DELETE -> NotificationActionKind.NOTIFICATION_ACTION_KIND_DELETE
            a.semanticAction == AndroidValues.SEMANTIC_ARCHIVE -> NotificationActionKind.NOTIFICATION_ACTION_KIND_ARCHIVE
            a.semanticAction == AndroidValues.SEMANTIC_MUTE -> NotificationActionKind.NOTIFICATION_ACTION_KIND_MUTE
            else -> NotificationActionKind.NOTIFICATION_ACTION_KIND_UNSPECIFIED
        }
    }

    private fun importanceOf(value: Int): NotificationImportance = when (value) {
        AndroidValues.IMPORTANCE_MIN -> NotificationImportance.NOTIFICATION_IMPORTANCE_MIN
        AndroidValues.IMPORTANCE_LOW -> NotificationImportance.NOTIFICATION_IMPORTANCE_LOW
        AndroidValues.IMPORTANCE_DEFAULT -> NotificationImportance.NOTIFICATION_IMPORTANCE_DEFAULT
        AndroidValues.IMPORTANCE_HIGH, AndroidValues.IMPORTANCE_MAX -> NotificationImportance.NOTIFICATION_IMPORTANCE_HIGH
        else -> NotificationImportance.NOTIFICATION_IMPORTANCE_UNSPECIFIED
    }

    private fun visibilityOf(value: Int): NotificationVisibility = when (value) {
        AndroidValues.VISIBILITY_PUBLIC -> NotificationVisibility.NOTIFICATION_VISIBILITY_PUBLIC
        AndroidValues.VISIBILITY_SECRET -> NotificationVisibility.NOTIFICATION_VISIBILITY_SECRET
        else -> NotificationVisibility.NOTIFICATION_VISIBILITY_PRIVATE
    }

    private fun categoryOf(s: NotificationSnapshot): NotificationCategory = when (s.category) {
        "msg" -> NotificationCategory.NOTIFICATION_CATEGORY_MESSAGE
        "email" -> NotificationCategory.NOTIFICATION_CATEGORY_EMAIL
        "call" -> NotificationCategory.NOTIFICATION_CATEGORY_CALL
        "missed_call" -> NotificationCategory.NOTIFICATION_CATEGORY_MISSED_CALL
        "event" -> NotificationCategory.NOTIFICATION_CATEGORY_EVENT
        "reminder" -> NotificationCategory.NOTIFICATION_CATEGORY_REMINDER
        "alarm" -> NotificationCategory.NOTIFICATION_CATEGORY_ALARM
        "progress" -> NotificationCategory.NOTIFICATION_CATEGORY_PROGRESS
        "transport" -> NotificationCategory.NOTIFICATION_CATEGORY_MEDIA
        "social" -> NotificationCategory.NOTIFICATION_CATEGORY_SOCIAL
        "promo" -> NotificationCategory.NOTIFICATION_CATEGORY_PROMOTION
        "navigation" -> NotificationCategory.NOTIFICATION_CATEGORY_NAVIGATION
        "err", "sys", "service", "status" -> NotificationCategory.NOTIFICATION_CATEGORY_SYSTEM
        else -> when {
            s.actions.any { it.callRole != null } -> NotificationCategory.NOTIFICATION_CATEGORY_CALL
            s.conversation != null -> NotificationCategory.NOTIFICATION_CATEGORY_MESSAGE
            s.progress != null -> NotificationCategory.NOTIFICATION_CATEGORY_PROGRESS
            else -> NotificationCategory.NOTIFICATION_CATEGORY_UNSPECIFIED
        }
    }
}
