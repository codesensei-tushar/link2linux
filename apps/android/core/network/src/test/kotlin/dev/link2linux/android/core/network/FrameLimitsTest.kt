package dev.link2linux.android.core.network

import com.google.protobuf.ByteString
import com.google.protobuf.InvalidProtocolBufferException
import dev.link2linux.protocol.v1.ConversationMessage
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.FeatureState
import dev.link2linux.protocol.v1.FileInfo
import dev.link2linux.protocol.v1.NotificationAction
import dev.link2linux.protocol.v1.PhoneNotification
import dev.link2linux.protocol.v1.Photo
import dev.link2linux.protocol.v1.ScreenPoint
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.fileOffer
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.imageData
import dev.link2linux.protocol.v1.notificationAction
import dev.link2linux.protocol.v1.notificationPosted
import dev.link2linux.protocol.v1.notificationSyncBatch
import dev.link2linux.protocol.v1.phoneNotification
import dev.link2linux.protocol.v1.photoListChanged
import dev.link2linux.protocol.v1.photoListReply
import dev.link2linux.protocol.v1.screenGesture
import dev.link2linux.protocol.v1.screenInput
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameLimitsTest {

    private fun frame(fill: Envelope.Builder.() -> Unit): ByteArray =
        Envelope.newBuilder().setSeq(1).apply(fill).build().toByteArray()

    private fun check(body: ByteArray, maxMessageBytes: Int = Int.MAX_VALUE) = FrameLimits.check(body, maxMessageBytes)

    private fun tooMany(body: ByteArray): String =
        (runCatching { check(body) }.exceptionOrNull() as TooManyEntriesException).field

    private fun batch(notifications: Int = 0, removedKeys: Int = 0) = frame {
        notificationSyncBatch = notificationSyncBatch {
            this.notifications += List(notifications) { PhoneNotification.getDefaultInstance() }
            this.removedKeys += List(removedKeys) { "" }
        }
    }

    private fun posted(notification: PhoneNotification) = frame {
        notificationPosted = notificationPosted { this.notification = notification }
    }

    private fun offer(files: Int) = frame { fileOffer = fileOffer { this.files += List(files) { FileInfo.getDefaultInstance() } } }

    private fun photos(photos: Int) = frame {
        photoListReply = photoListReply { this.photos += List(photos) { Photo.getDefaultInstance() } }
    }

    private fun gesture(points: Int) = frame {
        screenInput = screenInput {
            gesture = screenGesture {
                formatId = 1
                this.points += List(points) { ScreenPoint.getDefaultInstance() }
            }
        }
    }

    private val full = phoneNotification {
        actions += List(32) { notificationAction { replyChoices += List(16) { "ok" } } }
        messages += List(32) { ConversationMessage.getDefaultInstance() }
    }

    @Test
    fun framesAtEveryLimitPass() {
        check(frame { hello = hello { features += List(64) { "notifications" } } })
        check(frame { featureStatus = featureStatus { features += List(64) { FeatureState.getDefaultInstance() } } })
        check(batch(notifications = 100, removedKeys = 1000))
        check(posted(full))
        check(frame { notificationSyncBatch = notificationSyncBatch { notifications += List(100) { full } } })
        check(offer(500))
        check(photos(200))
        check(gesture(64))
    }

    @Test
    fun oneEntryTooManyIsRefused() {
        assertEquals("Hello.features", tooMany(frame { hello = hello { features += List(65) { "" } } }))
        assertEquals(
            "FeatureStatus.features",
            tooMany(frame { featureStatus = featureStatus { features += List(65) { FeatureState.getDefaultInstance() } } }),
        )
        assertEquals("PhoneNotification", tooMany(batch(notifications = 101)))
        assertEquals("NotificationSyncBatch.removed_keys", tooMany(batch(removedKeys = 1001)))
        assertEquals(
            "PhoneNotification.actions",
            tooMany(posted(phoneNotification { actions += List(33) { NotificationAction.getDefaultInstance() } })),
        )
        assertEquals(
            "PhoneNotification.messages",
            tooMany(posted(phoneNotification { messages += List(33) { ConversationMessage.getDefaultInstance() } })),
        )
        assertEquals(
            "NotificationAction.reply_choices",
            tooMany(posted(phoneNotification { actions += notificationAction { replyChoices += List(17) { "" } } })),
        )
        assertEquals("FileOffer.files", tooMany(offer(501)))
        // Offers merged from two copies count together.
        assertEquals("FileOffer.files", tooMany(offer(300) + offer(300)))
        assertEquals("Photo", tooMany(photos(201)))
        // The photos of a reply and of a change count together.
        val changed = frame { photoListChanged = photoListChanged { photos += List(51) { Photo.getDefaultInstance() } } }
        assertEquals("Photo", tooMany(photos(150) + changed))
        assertEquals("ScreenGesture.points", tooMany(gesture(65)))
    }

    @Test
    fun everyOccurrenceOfAMergedPayloadCounts() {
        // Two serialized envelopes back to back parse as one, whose batch
        // holds the notifications of both.
        val twice = batch(notifications = 60) + batch(notifications = 60)
        assertEquals(120, Envelope.parseFrom(twice).notificationSyncBatch.notificationsCount)
        assertEquals("PhoneNotification", tooMany(twice))
        // A posted notification counts against the same total.
        assertEquals("PhoneNotification", tooMany(batch(notifications = 100) + posted(PhoneNotification.getDefaultInstance())))
    }

    @Test
    fun onlyAnImageMayTakeMoreThanTheMessageLimit() {
        val image = frame { imageData = imageData { data = ByteString.copyFrom(ByteArray(1000)) } }
        check(image, maxMessageBytes = 100)
        val text = posted(phoneNotification { text = "x".repeat(1000) })
        assertEquals(100, (runCatching { check(text, 100) }.exceptionOrNull() as FrameTooLargeException).limit)
        // What is not the image still counts.
        val both = image + frame { hello = hello { deviceName = "x".repeat(200) } }
        assertTrue(runCatching { check(both, 100) }.exceptionOrNull() is FrameTooLargeException)
    }

    @Test
    fun bodiesThatCannotBeReadAreRefused() {
        for (body in listOf(
            "ff ff", // a key that never ends
            "08", // a varint field without its value
            "09 01 02 03", // a 64-bit field cut short
            "aa 06 05 00", // a batch longer than the frame
            "0b 0c", // a group
            "0e", // wire type 6
            "aa 06 02 12 05", // a batch whose notification is cut short
        )) {
            val error = runCatching { check(hex(body)) }.exceptionOrNull()
            assertTrue("$body: got $error", error is InvalidProtocolBufferException)
        }
    }

    @Test
    fun fieldsThatAreNotCountedAreSkipped() {
        check(frame { hello = hello { } } + hex("78 01 79 00 00 00 00 00 00 00 00 7a 01 aa 7d 00 00 00 00"))
    }
}
