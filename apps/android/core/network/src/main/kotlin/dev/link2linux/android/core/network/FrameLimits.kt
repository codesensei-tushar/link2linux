package dev.link2linux.android.core.network

import com.google.protobuf.InvalidProtocolBufferException

/**
 * The checks a frame passes before its body is parsed (docs/protocol.md,
 * "Framing"), the same as the desktop's.
 *
 * Parsing allocates for every entry of a repeated field, and an empty entry
 * takes two bytes on the wire but far more on the heap, so a frame well
 * within the size limit could need hundreds of MiB once parsed. [check]
 * reads the frame's wire format without allocating and refuses it when a
 * repeated field holds more entries than the protocol allows, or when the
 * frame is too large for a frame without an image. It reads into the
 * payloads that have repeated fields and skips everything else by its
 * length.
 *
 * A parser merges a message field that appears more than once, so the
 * entries of every occurrence count together.
 *
 * The limits inside a notification are looser than the protocol's content
 * limits, which a receiver applies after parsing; these only bound what
 * parsing can allocate.
 */
internal object FrameLimits {

    // Envelope payloads, by field number.
    private const val HELLO = 10
    private const val FEATURE_STATUS = 14
    private const val IMAGE_DATA = 18
    private const val NOTIFICATION_SYNC_BATCH = 101
    private const val NOTIFICATION_POSTED = 102
    private const val PHOTO_LIST_REPLY = 251
    private const val PHOTO_LIST_CHANGED = 252
    private const val FILE_OFFER = 300
    private const val SCREEN_INPUT = 406

    private class Limit(val field: String, val max: Int) {
        /** Counts one more entry after [count] and returns the new count. */
        fun add(count: Int): Int {
            if (count >= max) throw TooManyEntriesException(field, max)
            return count + 1
        }
    }

    private val FEATURES = Limit("Hello.features", 64)
    private val FEATURE_STATES = Limit("FeatureStatus.features", 64)

    /** Of a sync batch and `NotificationPosted` together. */
    private val NOTIFICATIONS = Limit("PhoneNotification", 100)
    private val REMOVED_KEYS = Limit("NotificationSyncBatch.removed_keys", 1000)

    /** Per notification. */
    private val ACTIONS = Limit("PhoneNotification.actions", 32)
    private val MESSAGES = Limit("PhoneNotification.messages", 32)

    /** Per action. */
    private val REPLY_CHOICES = Limit("NotificationAction.reply_choices", 16)

    private val FILES = Limit("FileOffer.files", 500)

    /** Of a `PhotoListReply` and `PhotoListChanged` together. */
    private val PHOTOS = Limit("Photo", 200)
    private val GESTURE_POINTS = Limit("ScreenGesture.points", 64)

    /**
     * Checks the frame body [body] (one serialized `Envelope`) before it is
     * parsed: the entries of its repeated fields, and that it is at most
     * [maxMessageBytes] long, not counting an `ImageData` payload.
     */
    fun check(body: ByteArray, maxMessageBytes: Int) {
        var features = 0
        var featureStates = 0
        var notifications = 0
        var removedKeys = 0
        var files = 0
        var photos = 0
        var gesturePoints = 0
        var imageBytes = 0
        forEachField(body, 0, body.size) { payload, start, end ->
            when (payload) {
                HELLO -> features = count(body, start, end, 5, FEATURES, features)
                FEATURE_STATUS -> featureStates = count(body, start, end, 1, FEATURE_STATES, featureStates)
                IMAGE_DATA -> imageBytes += end - start
                NOTIFICATION_SYNC_BATCH -> forEachField(body, start, end) { field, from, to ->
                    when (field) {
                        2 -> notifications = notification(body, from, to, notifications)
                        3 -> removedKeys = REMOVED_KEYS.add(removedKeys)
                    }
                }
                NOTIFICATION_POSTED -> forEachField(body, start, end) { field, from, to ->
                    if (field == 1) notifications = notification(body, from, to, notifications)
                }
                PHOTO_LIST_REPLY, PHOTO_LIST_CHANGED -> photos = count(body, start, end, 1, PHOTOS, photos)
                FILE_OFFER -> files = count(body, start, end, 3, FILES, files)
                SCREEN_INPUT -> forEachField(body, start, end) { field, from, to ->
                    if (field == 1) gesturePoints = count(body, from, to, 2, GESTURE_POINTS, gesturePoints)
                }
            }
        }
        val length = body.size - imageBytes
        if (length > maxMessageBytes) throw FrameTooLargeException(length.toLong(), maxMessageBytes)
    }

    /** Checks one `PhoneNotification` and returns [total] with it counted. */
    private fun notification(bytes: ByteArray, from: Int, to: Int, total: Int): Int {
        val counted = NOTIFICATIONS.add(total)
        var actions = 0
        var messages = 0
        forEachField(bytes, from, to) { field, start, end ->
            when (field) {
                14 -> {
                    actions = ACTIONS.add(actions)
                    count(bytes, start, end, 5, REPLY_CHOICES, 0)
                }
                15 -> messages = MESSAGES.add(messages)
            }
        }
        return counted
    }

    /** Returns [total] plus the entries of field [number] of the message in `bytes[from, to)`. */
    private fun count(bytes: ByteArray, from: Int, to: Int, number: Int, limit: Limit, total: Int): Int {
        var count = total
        forEachField(bytes, from, to) { field, _, _ -> if (field == number) count = limit.add(count) }
        return count
    }

    /**
     * Calls [visit] with the field number and bounds of each length-delimited
     * field of the message in `bytes[from, to)`, in wire order, and skips the
     * other fields.
     */
    private inline fun forEachField(bytes: ByteArray, from: Int, to: Int, visit: (Int, Int, Int) -> Unit) {
        val cursor = Cursor(bytes, from, to)
        while (cursor.position < to) {
            val key = cursor.varint()
            val number = key ushr 3
            if (number > Int.MAX_VALUE) throw malformed()
            when ((key and 7).toInt()) {
                0 -> cursor.varint()
                1 -> cursor.skip(8)
                2 -> {
                    val length = cursor.varint()
                    val start = cursor.position
                    cursor.skip(length)
                    visit(number.toInt(), start, cursor.position)
                }
                5 -> cursor.skip(4)
                // Groups (3 and 4) are not part of proto3; 6 and 7 are not wire types.
                else -> throw malformed()
            }
        }
    }

    private class Cursor(private val bytes: ByteArray, var position: Int, private val end: Int) {
        fun varint(): Long {
            var value = 0L
            for (index in 0 until 10) {
                if (position >= end) throw malformed()
                val byte = bytes[position++].toInt() and 0xff
                if (index == 9 && byte > 1) throw malformed()
                value = value or ((byte and 0x7f).toLong() shl (7 * index))
                if (byte and 0x80 == 0) return value
            }
            throw malformed()
        }

        fun skip(count: Long) {
            if (count < 0 || count > end - position) throw malformed()
            position += count.toInt()
        }
    }

    private fun malformed() = InvalidProtocolBufferException("Frame body is not a well-formed message")
}
