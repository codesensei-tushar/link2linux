package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ErrorReply
import dev.link2linux.protocol.v1.ImageData
import dev.link2linux.protocol.v1.ImageRequest
import dev.link2linux.protocol.v1.NotificationActionRequest
import dev.link2linux.protocol.v1.NotificationDismissRequest
import dev.link2linux.protocol.v1.NotificationPosted
import dev.link2linux.protocol.v1.NotificationRemoved
import dev.link2linux.protocol.v1.NotificationSyncBatch
import dev.link2linux.protocol.v1.NotificationSyncRequest
import dev.link2linux.protocol.v1.errorReply

/*
 * The seam between the notifications engine and the session. The session
 * implements [NotificationTransport] (it owns the envelopes and sequence
 * numbers) and calls [NotificationEngine.handle] for each request the desktop
 * sends, passing a [ReplySink] that wraps the request's `seq` as `reply_to`.
 */

/** A one-way event for the desktop. */
sealed interface NotificationEvent {
    data class Posted(val message: NotificationPosted) : NotificationEvent

    data class Removed(val message: NotificationRemoved) : NotificationEvent
}

/** Sends the engine's live events to the desktop of the current session. */
fun interface NotificationTransport {
    /** May throw when the connection is gone; the engine drops the event, and delta sync recovers. */
    suspend fun send(event: NotificationEvent)

    /**
     * The desktop has stopped reading: so much waits for it that the engine
     * gave up. End the session, so it syncs again when it reconnects. Must
     * not block.
     */
    fun abandon() = Unit
}

/** A request from the desktop, as the session decoded it. */
sealed interface NotificationRequest {
    data class Sync(val message: NotificationSyncRequest) : NotificationRequest

    data class Dismiss(val message: NotificationDismissRequest) : NotificationRequest

    data class Action(val message: NotificationActionRequest) : NotificationRequest

    data class Image(val message: ImageRequest) : NotificationRequest
}

/** An answer to a request. A sync request gets several [Batch] answers. */
sealed interface NotificationReply {
    /** `Ack`. */
    data object Ack : NotificationReply

    data class Error(val message: ErrorReply) : NotificationReply {
        val code: ErrorCode get() = message.code
    }

    data class Batch(val message: NotificationSyncBatch) : NotificationReply

    data class Image(val message: ImageData) : NotificationReply
}

/** Where the engine puts the answers to one request, in order. */
fun interface ReplySink {
    suspend fun send(reply: NotificationReply)
}

/** Whether the feature can be used right now, for `FeatureStatus` and for refusing requests. */
enum class NotificationAvailability {
    READY,

    /** The user has not granted notification access (`notification-access`). */
    PERMISSION_REQUIRED,

    /** The user turned the feature off on the phone. */
    DISABLED,
}

/** The user's choices, read when an event or request arrives. */
interface NotificationPolicy {
    /** The feature is turned on. */
    val enabled: Boolean

    /** Notification access is granted and the listener is connected. */
    val accessGranted: Boolean

    /** A dismissal on the desktop may cancel the notification on the phone. */
    val dismissSync: Boolean

    /** Notifications that look like they hold a one-time code are shared without their content. */
    val hideOneTimeCodes: Boolean

    /** The user excluded this app. */
    fun isExcluded(packageName: String): Boolean

    /** The user shares this app's notifications without their content. */
    fun isContentHidden(packageName: String): Boolean
}

/** Facts about the phone that decide whether an action may run now. */
interface NotificationEnvironment {
    /** The screen is locked. */
    val isLocked: Boolean

    /** The app may start an activity from the background: it has a companion association or is in the foreground. */
    val canStartActivitiesFromBackground: Boolean
}

/** Cancels a notification on the phone, as the platform's listener API allows. */
fun interface NotificationCanceller {
    /** @return false when the notification was not cancelled. */
    fun cancel(platformKey: String): Boolean
}

internal fun failure(code: ErrorCode, detail: String = "", permission: String = "") = NotificationReply.Error(
    errorReply {
        this.code = code
        this.detail = detail.take(1024)
        this.permission = permission
    },
)
