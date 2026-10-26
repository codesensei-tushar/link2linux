package dev.link2linux.android.feature.notifications

import android.app.Notification
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.Context
import android.content.pm.ApplicationInfo
import android.graphics.Bitmap
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Bundle
import android.os.Process
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import androidx.core.app.NotificationCompat
import androidx.core.os.BundleCompat
import dev.link2linux.android.core.notifications.ActionSnapshot
import dev.link2linux.android.core.notifications.AndroidValues
import dev.link2linux.android.core.notifications.CallRole
import dev.link2linux.android.core.notifications.ConversationSnapshot
import dev.link2linux.android.core.notifications.ImageSource
import dev.link2linux.android.core.notifications.MessageSnapshot
import dev.link2linux.android.core.notifications.NotificationSnapshot
import dev.link2linux.android.core.notifications.ProgressSnapshot
import dev.link2linux.android.core.notifications.ReplyInput
import java.util.concurrent.ConcurrentHashMap

/**
 * Copies a StatusBarNotification into the platform-neutral [NotificationSnapshot].
 * All decisions about what to share live in core:notifications; this class only
 * reads Android's fields, so it has no logic worth testing off a device.
 *
 * Unverified without a device: every field here, against real notifications.
 */
internal class SnapshotExtractor(private val context: Context) {
    private class AppInfo(val name: String, val icon: ImageSource)

    private val apps = ConcurrentHashMap<String, AppInfo>()

    fun clearCaches() = apps.clear()

    fun extract(sbn: StatusBarNotification, ranking: NotificationListenerService.Ranking?): NotificationSnapshot {
        val n = sbn.notification
        val extras = n.extras
        val app = appInfo(sbn, n)
        val style = NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(n)

        fun text(key: String): String = extras.getCharSequence(key)?.toString().orEmpty()

        val lines = extras.getCharSequenceArray(Notification.EXTRA_TEXT_LINES)
            ?.joinToString("\n") { it.toString() }.orEmpty()
        val progressMax = extras.getInt(Notification.EXTRA_PROGRESS_MAX, 0)
        val indeterminate = extras.getBoolean(Notification.EXTRA_PROGRESS_INDETERMINATE, false)

        return NotificationSnapshot(
            key = sbn.key,
            packageName = sbn.packageName,
            appName = app.name,
            postTimeMillis = sbn.postTime,
            eventTimeMillis = n.`when`,
            title = text(Notification.EXTRA_TITLE),
            bigTitle = text(Notification.EXTRA_TITLE_BIG),
            text = text(Notification.EXTRA_TEXT),
            bigText = text(Notification.EXTRA_BIG_TEXT).ifEmpty { lines },
            subText = text(Notification.EXTRA_SUB_TEXT),
            category = n.category,
            importance = ranking?.importance ?: AndroidValues.IMPORTANCE_UNSPECIFIED,
            isOngoing = n.flags and Notification.FLAG_ONGOING_EVENT != 0,
            isClearable = sbn.isClearable,
            isLocalOnly = n.flags and Notification.FLAG_LOCAL_ONLY != 0,
            // Another user or a work profile: its user handle is not ours.
            isFromOtherProfile = sbn.user != Process.myUserHandle(),
            groupKey = if (n.group != null) sbn.groupKey else null,
            isGroupSummary = n.flags and Notification.FLAG_GROUP_SUMMARY != 0,
            groupAlertBehavior = n.groupAlertBehavior,
            onlyAlertOnce = n.flags and Notification.FLAG_ONLY_ALERT_ONCE != 0,
            interruptionSuppressed = ranking != null && !ranking.matchesInterruptionFilter(),
            visibility = n.visibility,
            progress = if (extras.containsKey(Notification.EXTRA_PROGRESS_MAX) && (progressMax > 0 || indeterminate)) {
                ProgressSnapshot(extras.getInt(Notification.EXTRA_PROGRESS, 0), progressMax, indeterminate)
            } else {
                null
            },
            conversation = style?.let(::conversation),
            actions = callActions(extras) + n.actions.orEmpty().mapNotNull(::action),
            suggestedReplies = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                ranking?.smartReplies?.map { it.toString() }.orEmpty()
            } else {
                emptyList()
            },
            appIcon = app.icon,
            largeIcon = n.getLargeIcon()?.let { IconImageSource(context, it) },
            picture = picture(extras),
        )
    }

    /** The application info Android attaches to the notification, since package visibility can hide the app from a lookup. */
    private fun appInfo(sbn: StatusBarNotification, n: Notification): AppInfo = apps.getOrPut(sbn.packageName) {
        val pm = context.packageManager
        val info = BundleCompat.getParcelable(n.extras, EXTRA_BUILDER_APPLICATION_INFO, ApplicationInfo::class.java)
            ?: try {
                pm.getApplicationInfo(sbn.packageName, 0)
            } catch (_: Exception) {
                null
            }
        val name = info?.let { runCatching { it.loadLabel(pm).toString() }.getOrNull() }
            ?.takeIf { it.isNotBlank() } ?: sbn.packageName
        AppInfo(name, DrawableImageSource({ info?.let { runCatching { it.loadIcon(pm) }.getOrNull() } }, "app:${sbn.packageName}"))
    }

    private fun conversation(style: NotificationCompat.MessagingStyle): ConversationSnapshot {
        val user = style.user
        val messages = style.messages.map { m ->
            val person = m.person
            // A message without a sender, or from the user themselves, is one the phone's user sent.
            val outgoing = person == null ||
                (person.key != null && person.key == user.key) ||
                (person.key == null && user.key == null && person.name == user.name)
            MessageSnapshot(
                sender = if (outgoing) "" else person.name?.toString().orEmpty(),
                text = m.text?.toString().orEmpty(),
                timeMillis = m.timestamp,
                outgoing = outgoing,
            )
        }
        return ConversationSnapshot(
            messages = messages,
            title = style.conversationTitle?.toString().orEmpty(),
            isGroup = style.isGroupConversation,
        )
    }

    private fun action(a: Notification.Action): ActionSnapshot? {
        val intent = a.actionIntent ?: return null
        val input = a.remoteInputs?.firstOrNull()
        return ActionSnapshot(
            label = a.title?.toString().orEmpty(),
            semanticAction = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) a.semanticAction else AndroidValues.SEMANTIC_NONE,
            replyKey = input?.resultKey,
            reply = input?.let {
                ReplyInput(
                    allowFreeForm = it.allowFreeFormInput,
                    choices = it.choices?.map { c -> c.toString() }.orEmpty(),
                    allowGeneratedReplies = a.allowGeneratedReplies,
                )
            },
            requiresUnlock = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && a.isAuthenticationRequired,
            opensActivity = opensActivity(intent),
            invoker = PendingIntentInvoker(context, intent, input),
        )
    }

    /**
     * Before Android 12 a PendingIntent does not say what it starts, so it is taken as not
     * opening an activity; a background start the system blocks then fails when fired.
     */
    private fun opensActivity(intent: PendingIntent): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && intent.isActivity

    /** Answer, decline and hang-up of a call style notification, which Android keeps outside the action list. */
    private fun callActions(extras: Bundle): List<ActionSnapshot> {
        if (!extras.containsKey(EXTRA_CALL_TYPE)) return emptyList()
        fun call(key: String, role: CallRole): ActionSnapshot? {
            val intent = BundleCompat.getParcelable(extras, key, PendingIntent::class.java) ?: return null
            return ActionSnapshot(
                label = "",
                callRole = role,
                opensActivity = opensActivity(intent),
                invoker = PendingIntentInvoker(context, intent, null),
            )
        }
        return when (extras.getInt(EXTRA_CALL_TYPE)) {
            CALL_TYPE_INCOMING -> listOfNotNull(call(EXTRA_ANSWER_INTENT, CallRole.ANSWER), call(EXTRA_DECLINE_INTENT, CallRole.DECLINE))
            CALL_TYPE_ONGOING, CALL_TYPE_SCREENING -> listOfNotNull(call(EXTRA_HANG_UP_INTENT, CallRole.HANG_UP))
            else -> emptyList()
        }
    }

    private fun picture(extras: Bundle): ImageSource? {
        BundleCompat.getParcelable(extras, EXTRA_PICTURE_ICON, Icon::class.java)?.let { return IconImageSource(context, it) }
        return BundleCompat.getParcelable(extras, Notification.EXTRA_PICTURE, Bitmap::class.java)?.let(::BitmapImageSource)
    }

    private companion object {
        // Spelled out because the SDK either hides them or only has them from Android 12.
        const val EXTRA_BUILDER_APPLICATION_INFO = "android.appInfo"
        const val EXTRA_CALL_TYPE = "android.callType"
        const val EXTRA_ANSWER_INTENT = "android.answerIntent"
        const val EXTRA_DECLINE_INTENT = "android.declineIntent"
        const val EXTRA_HANG_UP_INTENT = "android.hangUpIntent"
        const val EXTRA_PICTURE_ICON = "android.pictureIcon"
        const val CALL_TYPE_INCOMING = 1
        const val CALL_TYPE_ONGOING = 2
        const val CALL_TYPE_SCREENING = 3
    }
}
