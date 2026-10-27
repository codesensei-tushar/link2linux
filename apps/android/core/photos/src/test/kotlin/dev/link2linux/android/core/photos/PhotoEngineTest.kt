package dev.link2linux.android.core.photos

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ImageFormat
import dev.link2linux.protocol.v1.Photo
import dev.link2linux.protocol.v1.photoDataRequest
import dev.link2linux.protocol.v1.photoThumbnailRequest
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class PhotoEngineTest {

    private val library = FakeLibrary(listOf(photo("b", taken = 2), photo("a", taken = 1)))
    private val failures = mutableListOf<Throwable>()

    private fun TestScope.engine(availability: PhotoAvailability = PhotoAvailability.READY) =
        PhotoEngine(backgroundScope, library, availability, onFailure = { failures += it })

    private fun ids(answer: PhotoAnswer): List<String> = (answer as PhotoAnswer.Listed).photos.map(Photo::getId)

    private fun code(answer: PhotoAnswer): ErrorCode = (answer as PhotoAnswer.Refused).message.code

    private suspend fun PhotoEngine.thumbnail(desktop: PhotoDesktop, id: String, maxSize: Int = 256) =
        thumbnail(desktop, photoThumbnailRequest {
            this.id = id
            this.maxSize = maxSize
        })

    private suspend fun PhotoEngine.data(desktop: PhotoDesktop, id: String, offset: Long = 0) =
        data(desktop, photoDataRequest {
            this.id = id
            this.offset = offset
        })

    /** Asks for the list as [desktop], which stays subscribed, and gives the reply; [changed] gets the updates. */
    private suspend fun TestScope.ask(
        engine: PhotoEngine,
        desktop: PhotoDesktop = PhotoDesktop(),
        changed: suspend (List<Photo>) -> Unit = {},
    ): PhotoAnswer {
        val reply = CompletableDeferred<PhotoAnswer>()
        backgroundScope.launch { engine.subscribe(desktop, { reply.complete(it) }, changed) }
        runCurrent()
        return reply.await()
    }

    /** Asks for the list and follows it, as a desktop does; gives what it is sent after the reply, with the time. */
    private suspend fun TestScope.subscribe(engine: PhotoEngine, desktop: PhotoDesktop): MutableList<Pair<Long, List<String>>> {
        val sent = mutableListOf<Pair<Long, List<String>>>()
        ask(engine, desktop) { sent += testScheduler.currentTime to it.map(Photo::getId) } as PhotoAnswer.Listed
        return sent
    }

    @Test
    fun theListIsTheLibrarysNewestFirst() = runTest {
        library.photos.value = listOf(photo("a", taken = 1), photo("c", taken = 3), photo("b", taken = 2))

        assertEquals(listOf("c", "b", "a"), ids(ask(engine())))
    }

    @Test
    fun theListKeepsWhatTheProtocolAllows() = runTest {
        library.photos.value = listOf(
            photo("ok", taken = 5, name = "../x.jpg"),
            photo("", taken = 4),
            photo("x".repeat(65), taken = 3),
            photo("ok", taken = 2, name = "twice.jpg"),
            photo("nameless", taken = 1, name = ""),
        )

        val listed = (ask(engine()) as PhotoAnswer.Listed).photos

        assertEquals(listOf("ok", "nameless"), listed.map(Photo::getId))
        assertEquals(listOf("___x.jpg", "photo"), listed.map(Photo::getName))
    }

    @Test
    fun theListHoldsAtMostTwoHundred() = runTest {
        library.photos.value = (250 downTo 1).map { photo("p$it", taken = it.toLong()) }

        val listed = ids(ask(engine()))

        assertEquals(200, listed.size)
        assertEquals("p250", listed.first())
        assertEquals("p51", listed.last())
    }

    @Test
    fun everyRequestIsRefusedWhileTheFeatureIsOff() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)
        engine.setAvailability(PhotoAvailability.DISABLED)

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, code(ask(engine, desktop)))
        assertEquals(ErrorCode.ERROR_CODE_DISABLED, code(engine.thumbnail(desktop, "a")))
        assertEquals(ErrorCode.ERROR_CODE_DISABLED, code(engine.data(desktop, "a")))
    }

    @Test
    fun withoutThePermissionRequestsNameIt() = runTest {
        val refused = (ask(engine(PhotoAvailability.PERMISSION_REQUIRED)) as PhotoAnswer.Refused).message

        assertEquals(ErrorCode.ERROR_CODE_PERMISSION_REQUIRED, refused.code)
        assertEquals("photos", refused.permission)
    }

    @Test
    fun aListThatFailsIsAnInternalError() = runTest {
        library.failure = SecurityException("no access")

        assertEquals(ErrorCode.ERROR_CODE_INTERNAL, code(ask(engine())))
        assertEquals(1, failures.size)
    }

    @Test
    fun onlyPhotosOfTheListSentToThatDesktopAreServed() = runTest {
        val engine = engine()
        val asked = PhotoDesktop()
        val other = PhotoDesktop()
        ask(engine, asked)

        assertTrue(engine.thumbnail(asked, "a") is PhotoAnswer.Thumbnail)
        // Another desktop, or another session, that did not ask has been sent nothing.
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(other, "a")))
        // An image the phone has but did not list.
        library.files["secret"] = ByteArray(3)
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.data(asked, "secret")))
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(asked, "secret")))
    }

    @Test
    fun aPhotoDeletedSinceIsNotFound() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        library.files["a"] = ByteArray(3)
        ask(engine, desktop)

        library.photos.value = listOf(photo("b", taken = 2))
        library.files.remove("a")

        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "a")))
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.data(desktop, "a")))
    }

    @Test
    fun aThumbnailSizeOutOfRangeIsInvalid() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)

        for (size in listOf(0, 63, 2049, -1)) {
            assertEquals("max_size $size", ErrorCode.ERROR_CODE_INVALID_ARGUMENT, code(engine.thumbnail(desktop, "a", size)))
        }
        assertTrue(engine.thumbnail(desktop, "a", 64) is PhotoAnswer.Thumbnail)
        assertTrue(engine.thumbnail(desktop, "a", 2048) is PhotoAnswer.Thumbnail)
    }

    @Test
    fun aThumbnailIsAJpegOfTheSizeAskedFor() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)

        val thumbnail = (engine.thumbnail(desktop, "a", 400) as PhotoAnswer.Thumbnail).message

        assertEquals(ImageFormat.IMAGE_FORMAT_JPEG, thumbnail.format)
        assertEquals(400 to 300, thumbnail.width to thumbnail.height)
        assertEquals(listOf(400 to 85), library.encoded)
        assertEquals(1, library.closed)
    }

    @Test
    fun aSmallPhotoIsNeverScaledUp() = runTest {
        library.width = 300
        library.height = 200
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)

        val thumbnail = (engine.thumbnail(desktop, "a", 2048) as PhotoAnswer.Thumbnail).message

        assertEquals(300 to 200, thumbnail.width to thumbnail.height)
    }

    @Test
    fun aLargeThumbnailLowersItsQualityToFit() = runTest {
        library.bytesPerPixel = 0.2
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)

        val thumbnail = (engine.thumbnail(desktop, "a", 2048) as PhotoAnswer.Thumbnail).message

        assertEquals(listOf(2048 to 85, 2048 to 70, 2048 to 55), library.encoded)
        assertEquals(2048 to 1536, thumbnail.width to thumbnail.height)
        assertTrue(thumbnail.data.size() <= PhotoEngine.MAX_THUMBNAIL_BYTES)
    }

    @Test
    fun aThumbnailThatStillDoesNotFitGetsSmaller() = runTest {
        library.bytesPerPixel = 1.0
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)

        val thumbnail = (engine.thumbnail(desktop, "a", 2048) as PhotoAnswer.Thumbnail).message

        assertEquals(1152 to 864, thumbnail.width to thumbnail.height)
        assertEquals(1152 to 40, library.encoded.last())
        assertTrue(thumbnail.data.size() <= PhotoEngine.MAX_THUMBNAIL_BYTES)
    }

    @Test
    fun theDataComesInChunksWithTheFilesSizeAndTime() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        val file = ByteArray(600 * 1024) { it.toByte() }
        library.files["a"] = file
        library.modified["a"] = 77
        ask(engine, desktop)

        val first = (engine.data(desktop, "a") as PhotoAnswer.Data).message
        val last = (engine.data(desktop, "a", 512L * 1024) as PhotoAnswer.Data).message
        val end = (engine.data(desktop, "a", file.size.toLong()) as PhotoAnswer.Data).message

        assertEquals(file.size.toLong(), first.size)
        assertEquals(77, first.modifiedUnixMs)
        assertEquals(file.copyOfRange(0, 256 * 1024).toList(), first.data.toByteArray().toList())
        assertEquals(file.copyOfRange(512 * 1024, file.size).toList(), last.data.toByteArray().toList())
        assertTrue(end.data.isEmpty)
    }

    @Test
    fun anOffsetPastTheEndIsInvalid() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        library.files["a"] = ByteArray(10)
        ask(engine, desktop)

        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, code(engine.data(desktop, "a", 11)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, code(engine.data(desktop, "a", -1)))
    }

    @Test
    fun tooManyThumbnailsWaitingAreBusy() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        ask(engine, desktop)
        val gate = CompletableDeferred<Unit>()
        library.gate = gate

        val waiting: List<Deferred<PhotoAnswer>> = (1..PhotoEngine.MAX_THUMBNAILS_WAITING).map {
            backgroundScope.async { engine.thumbnail(desktop, "a") }
        }
        runCurrent()
        val busy = engine.thumbnail(desktop, "a") as PhotoAnswer.Refused

        assertEquals(ErrorCode.ERROR_CODE_BUSY, busy.message.code)
        assertEquals(1_000, busy.message.retryAfterMs)
        gate.complete(Unit)
        assertTrue(waiting.all { it.await() is PhotoAnswer.Thumbnail })
        assertTrue(engine.thumbnail(desktop, "a") is PhotoAnswer.Thumbnail)
    }

    @Test
    fun tooManyReadsWaitingAreBusy() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        library.files["a"] = ByteArray(10)
        ask(engine, desktop)
        val gate = CompletableDeferred<Unit>()
        library.gate = gate

        val waiting = (1..PhotoEngine.MAX_DATA_WAITING).map { backgroundScope.async { engine.data(desktop, "a") } }
        runCurrent()

        assertEquals(ErrorCode.ERROR_CODE_BUSY, code(engine.data(desktop, "a")))
        gate.complete(Unit)
        assertTrue(waiting.all { it.await() is PhotoAnswer.Data })
    }

    @Test
    fun aChangeWaitsFiveSecondsFromTheReplyAndOnlyTheNewestGoesOut() = runTest {
        val engine = engine()
        val sent = subscribe(engine, PhotoDesktop())

        advanceTimeBy(1_000)
        library.photos.value = listOf(photo("c", taken = 3)) + library.photos.value
        advanceTimeBy(1_000)
        library.photos.value = listOf(photo("d", taken = 4)) + library.photos.value
        advanceTimeBy(2_999)
        runCurrent()
        assertEquals(emptyList<Pair<Long, List<String>>>(), sent)

        advanceTimeBy(1)
        runCurrent()
        assertEquals(listOf(5_000L to listOf("d", "c", "b", "a")), sent)
    }

    @Test
    fun afterAnUpdateTheNextOneWaitsAnotherFiveSeconds() = runTest {
        val engine = engine()
        val sent = subscribe(engine, PhotoDesktop())
        library.photos.value = listOf(photo("b", taken = 2))
        advanceTimeBy(5_000)
        runCurrent()

        library.photos.value = emptyList()
        advanceTimeBy(4_999)
        runCurrent()
        assertEquals(listOf(5_000L to listOf("b")), sent)

        advanceTimeBy(1)
        runCurrent()
        assertEquals(listOf(5_000L to listOf("b"), 10_000L to emptyList()), sent)
    }

    @Test
    fun nothingGoesOutWhenTheListChangedBack() = runTest {
        val engine = engine()
        val first = library.photos.value
        val sent = subscribe(engine, PhotoDesktop())

        library.photos.value = listOf(photo("c", taken = 3)) + first
        advanceTimeBy(1_000)
        library.photos.value = first
        advanceTimeBy(30_000)
        runCurrent()

        assertEquals(emptyList<Pair<Long, List<String>>>(), sent)
    }

    @Test
    fun aStaleSharedListDoesNotReplaceTheReply() = runTest {
        val engine = engine()
        subscribe(engine, PhotoDesktop())
        // The shared flow keeps the list it loaded. The library moves on
        // without telling it, as when the user picks other photos.
        library.publishChanges = false
        library.photos.value = listOf(photo("c", taken = 3))
        val desktop = PhotoDesktop()
        val sent = subscribe(engine, desktop)

        advanceTimeBy(5_000)
        runCurrent()

        assertEquals(emptyList<Pair<Long, List<String>>>(), sent)
        assertTrue(engine.thumbnail(desktop, "c") is PhotoAnswer.Thumbnail)
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "b")))

        library.photos.value = listOf(photo("d", taken = 4))
        engine.refresh()
        runCurrent()
        advanceTimeBy(5_000)
        runCurrent()

        assertEquals(listOf(10_000L to listOf("d")), sent)
        assertTrue(engine.thumbnail(desktop, "d") is PhotoAnswer.Thumbnail)
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "c")))
    }

    @Test
    fun aNewVersionOfAPhotoIsAChange() = runTest {
        val engine = engine()
        val sent = subscribe(engine, PhotoDesktop())

        library.photos.value = listOf(photo("b", taken = 2, modified = 9), photo("a", taken = 1))
        advanceTimeBy(5_000)
        runCurrent()

        assertEquals(listOf(5_000L to listOf("b", "a")), sent)
    }

    @Test
    fun anUpdateChangesWhatTheDesktopMayFetch() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        subscribe(engine, desktop)

        library.photos.value = listOf(photo("c", taken = 3), photo("b", taken = 2))
        advanceTimeBy(5_000)
        runCurrent()

        assertTrue(engine.thumbnail(desktop, "c") is PhotoAnswer.Thumbnail)
        // Still on the phone, but no longer in the list the desktop has.
        library.photos.value = listOf(photo("c", taken = 3), photo("b", taken = 2), photo("a", taken = 1))
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "a")))
    }

    @Test
    fun theSubscriptionEndsWhenTheFeatureLeavesReady() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        val sent = subscribe(engine, desktop)

        engine.setAvailability(PhotoAvailability.PERMISSION_REQUIRED)
        runCurrent()
        library.photos.value = listOf(photo("c", taken = 3))
        advanceTimeBy(60_000)
        runCurrent()
        engine.setAvailability(PhotoAvailability.READY)

        assertEquals(emptyList<Pair<Long, List<String>>>(), sent)
        // Until it asks again, the desktop may fetch nothing.
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "c")))
        assertEquals(0, library.listeners)
    }

    @Test
    fun askingAgainReplacesTheFollower() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        val sent = mutableListOf<List<String>>()
        val first = backgroundScope.launch { engine.subscribe(desktop, {}) { sent += it.map(Photo::getId) } }
        runCurrent()

        library.photos.value = listOf(photo("c", taken = 3))
        ask(engine, desktop)
        advanceTimeBy(5_000)
        runCurrent()

        // The first follower saw that the desktop has a newer list, and stopped without touching it.
        assertTrue(first.isCompleted)
        assertEquals(emptyList<List<String>>(), sent)
        assertTrue(engine.thumbnail(desktop, "c") is PhotoAnswer.Thumbnail)
    }

    @Test
    fun aCancelledSubscriptionEndsWhatTheDesktopMayFetch() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        val subscription = backgroundScope.launch { engine.subscribe(desktop, {}, {}) }
        runCurrent()
        assertTrue(engine.thumbnail(desktop, "a") is PhotoAnswer.Thumbnail)

        subscription.cancel()
        runCurrent()

        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "a")))
    }

    @Test
    fun aSubscriptionCancelledWhileItRepliesEndsToo() = runTest {
        val engine = engine()
        val desktop = PhotoDesktop()
        val replying = CompletableDeferred<Unit>()
        val subscription = backgroundScope.launch { engine.subscribe(desktop, { replying.await() }, {}) }
        runCurrent()

        subscription.cancel()
        runCurrent()

        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, code(engine.thumbnail(desktop, "a")))
    }

    @Test
    fun theListenerRunsWhileADesktopFollowsAndAWhileAfter() = runTest {
        val engine = engine()
        val subscription = backgroundScope.launch { engine.subscribe(PhotoDesktop(), {}, {}) }
        runCurrent()
        assertEquals(1, library.listeners)

        subscription.cancel()
        advanceTimeBy(PhotoEngine.KEEP_LISTENING_MILLIS - 1)
        runCurrent()
        assertEquals(1, library.listeners)
        advanceTimeBy(1)
        runCurrent()
        assertEquals(0, library.listeners)
    }

    @Test
    fun aRefreshLoadsTheListAgain() = runTest {
        val engine = engine()
        subscribe(engine, PhotoDesktop())
        val loads = library.loads

        engine.refresh()
        runCurrent()

        assertEquals(loads + 1, library.loads)
    }

    @Test
    fun aListThatFailsWhileFollowingIsLoadedAgain() = runTest {
        val engine = engine()
        val sent = subscribe(engine, PhotoDesktop())
        library.failure = IllegalStateException("gone")
        library.photos.value = listOf(photo("c", taken = 3))
        runCurrent()
        assertEquals(1, failures.size)

        library.failure = null
        advanceTimeBy(PhotoEngine.RETRY_MILLIS + PhotoEngine.CHANGE_INTERVAL_MILLIS)
        runCurrent()

        assertEquals(listOf("c"), sent.single().second)
    }

    @Test
    fun everyChangeOfAvailabilityIsReportedInOrder() = runTest {
        val engine = engine()
        val changes = mutableListOf<PhotoAvailability>()
        backgroundScope.launch { engine.availabilityChanges.collect { changes += it } }
        runCurrent()

        engine.setAvailability(PhotoAvailability.DISABLED)
        engine.setAvailability(PhotoAvailability.DISABLED)
        engine.setAvailability(PhotoAvailability.READY)
        runCurrent()

        assertEquals(listOf(PhotoAvailability.DISABLED, PhotoAvailability.READY), changes)
        assertEquals(PhotoAvailability.READY, engine.availability.value)
    }
}
