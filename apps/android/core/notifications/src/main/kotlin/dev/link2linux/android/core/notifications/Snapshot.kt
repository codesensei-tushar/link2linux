package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ImageFormat

/**
 * One notification as the Android adapter read it, with no Android types. The
 * integer fields keep the platform's own constants (see [AndroidValues]) so the
 * adapter stays a thin copy and the mapping rules live here, under test.
 */
data class NotificationSnapshot(
    /** `StatusBarNotification.getKey()`. */
    val key: String,
    val packageName: String,
    val appName: String = packageName,
    val postTimeMillis: Long = 0,
    /** `Notification.when`; 0 when the app did not set one. */
    val eventTimeMillis: Long = 0,
    val title: String = "",
    /** The expanded title, when the style has one. */
    val bigTitle: String = "",
    val text: String = "",
    /** The expanded body, when the style has one. */
    val bigText: String = "",
    val subText: String = "",
    /** `Notification.category`. */
    val category: String? = null,
    /** The ranking's importance, as `NotificationManager.IMPORTANCE_*`. */
    val importance: Int = AndroidValues.IMPORTANCE_UNSPECIFIED,
    val isOngoing: Boolean = false,
    /** `StatusBarNotification.isClearable()`: a listener may cancel it. */
    val isClearable: Boolean = true,
    /** The app marked it as not to be bridged to other devices. */
    val isLocalOnly: Boolean = false,
    /** Posted for a work profile or another user. */
    val isFromOtherProfile: Boolean = false,
    /** The group key, or null when the app set no group. */
    val groupKey: String? = null,
    val isGroupSummary: Boolean = false,
    /** `Notification.GROUP_ALERT_*`. */
    val groupAlertBehavior: Int = AndroidValues.GROUP_ALERT_ALL,
    val onlyAlertOnce: Boolean = false,
    /** Android muted its interruption, for example under Do Not Disturb. */
    val interruptionSuppressed: Boolean = false,
    /** `Notification.visibility`: the app's choice. */
    val visibility: Int = AndroidValues.VISIBILITY_PRIVATE,
    /**
     * The user's lock-screen setting for the notification's channel, or
     * [AndroidValues.VISIBILITY_NO_OVERRIDE]. The stricter of the two is sent.
     */
    val visibilityOverride: Int = AndroidValues.VISIBILITY_NO_OVERRIDE,
    val progress: ProgressSnapshot? = null,
    val conversation: ConversationSnapshot? = null,
    /** Actions in display order, including call actions from a call style. */
    val actions: List<ActionSnapshot> = emptyList(),
    /** Replies Android suggests for reply actions that allow generated ones. */
    val suggestedReplies: List<String> = emptyList(),
    val appIcon: ImageSource? = null,
    val largeIcon: ImageSource? = null,
    val picture: ImageSource? = null,
)

data class ProgressSnapshot(val current: Int, val max: Int, val indeterminate: Boolean)

data class ConversationSnapshot(
    /** Oldest first. */
    val messages: List<MessageSnapshot>,
    val title: String = "",
    val isGroup: Boolean = false,
)

data class MessageSnapshot(
    /** Empty for messages the phone's user sent. */
    val sender: String,
    val text: String,
    val timeMillis: Long,
    val outgoing: Boolean,
)

/** What the app's own call style adds next to the regular action list. */
enum class CallRole { ANSWER, DECLINE, HANG_UP }

/** The free-text input of a reply action. */
data class ReplyInput(
    val allowFreeForm: Boolean = true,
    val choices: List<String> = emptyList(),
    val allowGeneratedReplies: Boolean = false,
)

data class ActionSnapshot(
    val label: String,
    /** `Notification.Action.SEMANTIC_ACTION_*`. */
    val semanticAction: Int = AndroidValues.SEMANTIC_NONE,
    /** Result key of the reply input; part of the action's id. */
    val replyKey: String? = null,
    val reply: ReplyInput? = null,
    /** Android runs it only on an unlocked phone. */
    val requiresUnlock: Boolean = false,
    /** Its PendingIntent starts an activity. */
    val opensActivity: Boolean = false,
    /** It may start an activity: it does, or Android cannot say (before Android 12). */
    val mayOpenActivity: Boolean = opensActivity,
    val callRole: CallRole? = null,
    val invoker: ActionInvoker,
)

/** What happened when an action's PendingIntent was fired. */
enum class ActionOutcome {
    SENT,

    /** The app cancelled the PendingIntent, so the action is gone. */
    CANCELED,

    /** The system refused to run it from here. */
    BLOCKED,
    FAILED,
}

/** Fires an action, with the reply text for actions that take one. */
fun interface ActionInvoker {
    fun invoke(replyText: String?, allowBackgroundStart: Boolean): ActionOutcome
}

/** Sizes from the protocol's "Icons and pictures". */
enum class ImageKind(val maxEdge: Int) {
    APP_ICON(128),
    LARGE_ICON(256),
    PICTURE(1024),
}

class EncodedImage(
    val format: ImageFormat,
    val data: ByteArray,
    val width: Int,
    val height: Int,
)

/** An image the adapter can render on demand. Rendering may be slow. */
interface ImageSource {
    /** Same key, same pixels: lets the store skip rendering again. Null when unknown. */
    val cacheKey: String? get() = null

    fun render(kind: ImageKind): EncodedImage?
}

/** `android.*` constants the mapping reads, copied so no Android classes are needed. */
object AndroidValues {
    const val IMPORTANCE_UNSPECIFIED = -1000
    const val IMPORTANCE_NONE = 0
    const val IMPORTANCE_MIN = 1
    const val IMPORTANCE_LOW = 2
    const val IMPORTANCE_DEFAULT = 3
    const val IMPORTANCE_HIGH = 4
    const val IMPORTANCE_MAX = 5

    const val VISIBILITY_SECRET = -1
    const val VISIBILITY_PRIVATE = 0
    const val VISIBILITY_PUBLIC = 1
    const val VISIBILITY_NO_OVERRIDE = -1000

    const val GROUP_ALERT_ALL = 0
    const val GROUP_ALERT_SUMMARY = 1
    const val GROUP_ALERT_CHILDREN = 2

    const val SEMANTIC_NONE = 0
    const val SEMANTIC_REPLY = 1
    const val SEMANTIC_MARK_AS_READ = 2
    const val SEMANTIC_DELETE = 3
    const val SEMANTIC_ARCHIVE = 4
    const val SEMANTIC_MUTE = 5
}
