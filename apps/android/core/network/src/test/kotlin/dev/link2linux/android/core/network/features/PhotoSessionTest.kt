package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.network.pairing.DesktopPeer
import dev.link2linux.android.core.network.pairing.PairingService
import dev.link2linux.android.core.network.tls.TestDesktop
import dev.link2linux.android.core.network.tls.asSource
import dev.link2linux.android.core.photos.EncodedImage
import dev.link2linux.android.core.photos.PhotoAvailability
import dev.link2linux.android.core.photos.PhotoBytes
import dev.link2linux.android.core.photos.PhotoEngine
import dev.link2linux.android.core.photos.PhotoLibrary
import dev.link2linux.android.core.photos.ScaledPhoto
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.Photo
import dev.link2linux.protocol.v1.PhotoListRequest
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.photo
import dev.link2linux.protocol.v1.photoDataRequest
import dev.link2linux.protocol.v1.photoThumbnailRequest
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

/**
 * The photos feature over real loopback TLS sessions, with a fake platform
 * behind the engine. The engine's own tests cover its pacing and its
 * answers; here it sends list updates 100 ms apart.
 */
class PhotoSessionTest {

    /** The platform's photos as a value the test sets; each has a small image and a file of 300 KiB. */
    private class FakeLibrary : PhotoLibrary {
        val photos = MutableStateFlow(listOf(photo("b", 2), photo("a", 1)))
        val file = ByteArray(300 * 1024) { it.toByte() }

        /** While set, decoding waits for it. */
        @Volatile
        var gate: CompletableDeferred<Unit>? = null

        override fun changes(): Flow<Unit> = photos.map { }

        override suspend fun photos(limit: Int): List<Photo> = photos.value

        override suspend fun decode(photo: Photo, maxSize: Int): ScaledPhoto {
            gate?.await()
            return object : ScaledPhoto {
                override val width = minOf(maxSize, 400)
                override val height = minOf(maxSize, 300)

                override fun jpeg(longestSide: Int, quality: Int) = EncodedImage(ByteArray(100), width, height)

                override fun close() = Unit
            }
        }

        override suspend fun read(photo: Photo, offset: Long, maxBytes: Int): PhotoBytes {
            val from = minOf(offset, file.size.toLong()).toInt()
            val to = minOf(file.size.toLong(), offset + maxBytes).toInt().coerceAtLeast(from)
            return PhotoBytes(file.size.toLong(), 5, file.copyOfRange(from, to))
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val library = FakeLibrary()
    private val engine = PhotoEngine(
        scope = scope,
        library = library,
        initialAvailability = PhotoAvailability.READY,
        intervalMillis = INTERVAL_MILLIS,
    )
    private val devices = InMemoryPairedDeviceRepository()
    private val service = PairingService(
        identity = TestCertificates.generate("test phone").asSource(),
        devices = devices,
        localDevice = object : LocalDeviceRepository {
            override suspend fun localDevice() = Device(DeviceId("phone-1"), "Phone", DeviceKind.PHONE)
            override suspend fun setName(name: String?) = Unit
        },
        scope = scope,
        listenAddresses = ListenAddresses { local, _ -> local.isLoopbackAddress },
        hooks = PhotoSessionHooks(engine, scope),
        port = 0,
        bindAddress = InetAddress.getLoopbackAddress(),
    )
    private val desktop = TestDesktop()

    @After
    fun tearDown() {
        runBlocking { service.stop() }
        scope.cancel()
    }

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    /** Pairs the desktop in the store if needed and connects it, listing [features] in its Hello. */
    private suspend fun connect(features: List<String> = listOf("photos")): DesktopPeer {
        val key = checkNotNull(desktop.fingerprint)
        if (devices.find(key) == null) {
            devices.save(
                PairedDevice(
                    device = Device(DeviceId(DesktopPeer.DEVICE_ID), "Test desktop", DeviceKind.DESKTOP),
                    fingerprint = key,
                    pairedAtEpochMillis = 0,
                    presenceKey = PresenceKey.of(ByteArray(32) { 9 }),
                ),
            )
        }
        service.start()
        val port = withTimeout(5_000) { service.listeningPort.filterNotNull().first() }
        return io {
            DesktopPeer(desktop.connect(port), desktop).apply {
                sendHello(PairingMethod.PAIRING_METHOD_UNSPECIFIED, features = features)
                assertEquals(listOf("photos"), readHello().featuresList)
            }
        }
    }

    /** Reads `FeatureStatus` and `PresenceUpdate`, and returns the status's part for photos. */
    private suspend fun DesktopPeer.readStart(): Envelope = io {
        val status = read()!!
        assertEquals(PayloadCase.PRESENCE_UPDATE, read()!!.payloadCase)
        status
    }

    private fun Envelope.availability(): FeatureAvailability {
        assertEquals(PayloadCase.FEATURE_STATUS, payloadCase)
        return featureStatus.featuresList.single { it.feature == "photos" }.availability
    }

    private suspend fun DesktopPeer.sendAvailability(availability: FeatureAvailability) = io {
        send {
            featureStatus = featureStatus {
                features += featureState {
                    feature = "photos"
                    this.availability = availability
                }
            }
        }
    }

    /** Asks for the list and returns the answer. */
    private suspend fun DesktopPeer.requestList(): Envelope = io {
        val request = send {
            expectsReply = true
            photoListRequest = PhotoListRequest.getDefaultInstance()
        }
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    private suspend fun DesktopPeer.requestThumbnail(id: String): Envelope = io {
        val request = sendThumbnailRequest(id)
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    private fun DesktopPeer.sendThumbnailRequest(id: String): Long = send {
        expectsReply = true
        photoThumbnailRequest = photoThumbnailRequest {
            this.id = id
            maxSize = 256
        }
    }

    private suspend fun DesktopPeer.requestData(id: String, offset: Long = 0): Envelope = io {
        val request = send {
            expectsReply = true
            photoDataRequest = photoDataRequest {
                this.id = id
                this.offset = offset
            }
        }
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    private suspend fun DesktopPeer.readChanged(): List<String> = io {
        read()!!.also { assertEquals(PayloadCase.PHOTO_LIST_CHANGED, it.payloadCase) }.photoListChanged.photosList.map(Photo::getId)
    }

    /** Waits past a few update intervals, then pings and checks that the `Pong` comes next: nothing else was sent. */
    private suspend fun DesktopPeer.assertNothingSent() {
        delay(4 * INTERVAL_MILLIS)
        io {
            send { ping = ping { nonce = 7 } }
            assertEquals(PayloadCase.PONG, read()!!.payloadCase)
        }
    }

    @Test
    fun theStatusFollowsTheSwitchAndThePermission() = runBlocking {
        engine.setAvailability(PhotoAvailability.DISABLED)
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, peer.readStart().availability())

        engine.setAvailability(PhotoAvailability.PERMISSION_REQUIRED)
        val status = io { peer.read()!! }
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED, status.availability())
        assertEquals("photos", status.featureStatus.featuresList.single().permission)

        engine.setAvailability(PhotoAvailability.READY)
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, io { peer.read()!! }.availability())
    }

    @Test
    fun aListRequestIsAnsweredAndChangesFollow() = runBlocking {
        val peer = connect()
        peer.readStart()

        val reply = peer.requestList()
        assertEquals(listOf("b", "a"), reply.photoListReply.photosList.map(Photo::getId))

        library.photos.value = listOf(photo("c", 3)) + library.photos.value
        assertEquals(listOf("c", "b", "a"), peer.readChanged())
    }

    @Test
    fun listedPhotosAreServed() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.requestList()

        val thumbnail = peer.requestThumbnail("a")
        assertEquals(PayloadCase.PHOTO_THUMBNAIL, thumbnail.payloadCase)
        assertEquals(256 to 256, thumbnail.photoThumbnail.width to thumbnail.photoThumbnail.height)

        val data = peer.requestData("a")
        assertEquals(PayloadCase.PHOTO_DATA, data.payloadCase)
        assertEquals(300L * 1024, data.photoData.size)
        assertEquals(256 * 1024, data.photoData.data.size())
        assertEquals(44 * 1024, peer.requestData("a", 256L * 1024).photoData.data.size())
    }

    @Test
    fun nothingIsServedBeforeTheListOrOutsideIt() = runBlocking {
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, peer.requestThumbnail("a").errorReply.code)
        peer.requestList()
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, peer.requestThumbnail("elsewhere").errorReply.code)
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, peer.requestData("elsewhere").errorReply.code)
    }

    @Test
    fun requestsWhileTheFeatureIsOffAreRefused() = runBlocking {
        engine.setAvailability(PhotoAvailability.DISABLED)
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, peer.requestList().errorReply.code)
        assertEquals(ErrorCode.ERROR_CODE_DISABLED, peer.requestThumbnail("a").errorReply.code)
    }

    @Test
    fun theSubscriptionEndsWhenTheDesktopIsNoLongerReady() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.sendAvailability(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        peer.requestList()

        peer.sendAvailability(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED)
        // The phone handles the desktop's messages in order, so the ping orders this check after the status.
        peer.assertNothingSent()
        library.photos.value = listOf(photo("c", 3))
        peer.assertNothingSent()
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, peer.requestThumbnail("c").errorReply.code)

        // Ready again: the desktop asks again, and the updates resume.
        peer.sendAvailability(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        assertEquals(listOf("c"), peer.requestList().photoListReply.photosList.map(Photo::getId))
        library.photos.value = listOf(photo("d", 4), photo("c", 3))
        assertEquals(listOf("d", "c"), peer.readChanged())
    }

    @Test
    fun turningTheFeatureOffEndsTheSubscription() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.requestList()

        engine.setAvailability(PhotoAvailability.DISABLED)
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, io { peer.read()!! }.availability())
        library.photos.value = listOf(photo("c", 3))

        peer.assertNothingSent()
    }

    @Test
    fun aDesktopWithTooManyWaitingGetsBusy() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.requestList()
        val gate = CompletableDeferred<Unit>()
        library.gate = gate

        val sent = io { (1..20).map { peer.sendThumbnailRequest("a") } }
        // The extra requests are answered at once; the others wait for the gate.
        val busy = io { (1..10).map { peer.read()!! } }
        assertTrue(busy.all { it.errorReply.code == ErrorCode.ERROR_CODE_BUSY && it.errorReply.retryAfterMs > 0 })
        gate.complete(Unit)
        val answers = busy + io { (1..10).map { peer.read()!! } }

        assertEquals(sent.toSet(), answers.map { it.replyTo }.toSet())
        assertEquals(10, answers.count { it.payloadCase == PayloadCase.PHOTO_THUMBNAIL })
    }

    @Test
    fun aRequestSentAsAnEventIsIgnored() = runBlocking {
        val peer = connect()
        peer.readStart()

        io { peer.send { photoListRequest = PhotoListRequest.getDefaultInstance() } }
        library.photos.value = listOf(photo("c", 3))

        peer.assertNothingSent()
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupported() = runBlocking {
        val peer = connect(features = listOf("notifications"))
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, peer.requestList().errorReply.code)
    }

    private companion object {
        const val INTERVAL_MILLIS = 100L

        fun photo(id: String, taken: Long): Photo = photo {
            this.id = id
            takenUnixMs = taken
            modifiedUnixMs = taken
            name = "$id.jpg"
            size = 300L * 1024
        }
    }
}
