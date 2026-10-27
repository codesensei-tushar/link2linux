package dev.link2linux.android.core.common

/**
 * Every notification the app posts, in one place. Notifications with no tag
 * share one slot per ID, and the foreground service's notification owns its
 * ID: an app notification posted under the same ID replaces it and is then
 * treated as the service's own, which Android neither cancels nor lets time
 * out. So each notification gets its own ID here, and a new one must be
 * added to [untagged] (or use a tag of its own).
 */
object NotificationIds {
    /** The foreground notification of `ConnectionService`. */
    const val CONNECTION = 1

    /** The notification shown while the phone rings. */
    const val RING = 2

    /** The prompt to pair a computer over Bluetooth. */
    const val PAIRING = 3

    /** The tag of the file transfer notifications, whose IDs come from the transfer. */
    const val FILES_TAG = "files"

    /** The IDs of the notifications posted without a tag. */
    val untagged: List<Int> = listOf(CONNECTION, RING, PAIRING)
}
