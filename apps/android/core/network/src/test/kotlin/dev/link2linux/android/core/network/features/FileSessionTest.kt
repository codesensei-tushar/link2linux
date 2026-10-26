package dev.link2linux.android.core.network.features

import com.google.protobuf.ByteString
import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.files.FileEngine
import dev.link2linux.android.core.files.FilePeer
import dev.link2linux.android.core.files.FilePolicy
import dev.link2linux.android.core.files.OutgoingFile
import dev.link2linux.android.core.files.PartialFile
import dev.link2linux.android.core.files.ReceivedFileStore
import dev.link2linux.android.core.files.SavedFile
import dev.link2linux.android.core.files.TransferState
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.network.pairing.DesktopPeer
import dev.link2linux.android.core.network.pairing.PairingService
import dev.link2linux.android.core.network.tls.TestDesktop
import dev.link2linux.android.core.network.tls.asSource
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.fileAccept
import dev.link2linux.protocol.v1.fileChunk
import dev.link2linux.protocol.v1.fileInfo
import dev.link2linux.protocol.v1.fileOffer
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.net.InetAddress
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** The files feature over real loopback TLS sessions, with storage and the files to send in memory. */
class FileSessionTest {

    private class Policy : FilePolicy {
        @Volatile override var enabled = true

        @Volatile override var storageGranted = true
    }

    private class MemoryStore : ReceivedFileStore {
        val saved = ConcurrentHashMap<String, ByteArray>()

        /** While set, writing waits for it, as slow storage would. */
        @Volatile var gate: CountDownLatch? = null

        override fun freeBytes() = Long.MAX_VALUE / 2

        override fun create(name: String, mimeType: String, size: Long): PartialFile = object : PartialFile {
            var bytes = ByteArray(0)

            override fun append(data: ByteArray) {
                gate?.await(10, TimeUnit.SECONDS)
                bytes += data
            }

            override fun publish(): SavedFile {
                saved[name] = bytes
                return SavedFile(name, "content://test/$name", "")
            }

            override fun delete() = Unit
        }

        override fun removeLeftovers() = Unit
    }

    private class MemoryFile(override val name: String, private val bytes: ByteArray) : OutgoingFile {
        override val size = bytes.size.toLong()
        override val mimeType = ""
        override val modifiedUnixMs = 0L

        override fun open() = ByteArrayInputStream(bytes)
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val policy = Policy()
    private val store = MemoryStore()
    private val engine = FileEngine(scope, policy, store, onError = { it.printStackTrace() })
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
        hooks = CompositeSessionHooks(FileSessionHooks(engine, scope)),
        port = 0,
        bindAddress = InetAddress.getLoopbackAddress(),
    )
    private val desktop = TestDesktop()
    private val peer = FilePeer(DesktopPeer.DEVICE_ID, "Test desktop")

    @After
    fun tearDown() {
        runBlocking { service.stop() }
        scope.cancel()
    }

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    /** Pairs the desktop in the store if needed and connects it, listing [features] in its Hello. */
    private suspend fun connect(features: List<String> = listOf("files")): DesktopPeer {
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
                assertEquals(listOf("files"), readHello().featuresList)
            }
        }
    }

    /** Reads `FeatureStatus` and `PresenceUpdate`, and returns the phone's state of files. */
    private suspend fun DesktopPeer.readStart(): Envelope = io {
        val status = read()!!
        assertEquals(PayloadCase.PRESENCE_UPDATE, read()!!.payloadCase)
        status
    }

    private fun Envelope.files() = featureStatus.featuresList.single { it.feature == "files" }

    private suspend fun DesktopPeer.sendStatus(availability: FeatureAvailability) = io {
        send {
            featureStatus = featureStatus {
                features += featureState {
                    feature = "files"
                    this.availability = availability
                }
            }
        }
    }

    private suspend fun DesktopPeer.request(fill: Envelope.Builder.() -> Unit): Envelope = io {
        val seq = send {
            expectsReply = true
            fill()
        }
        read()!!.also { assertEquals(seq, it.replyTo) }
    }

    private suspend fun DesktopPeer.readRequest(payload: PayloadCase): Envelope = io {
        read()!!.also {
            assertEquals(payload, it.payloadCase)
            assertTrue(it.expectsReply)
        }
    }

    private suspend fun DesktopPeer.reply(to: Envelope, fill: Envelope.Builder.() -> Unit) = io {
        send {
            replyTo = to.seq
            fill()
        }
    }

    private fun sha256(bytes: ByteArray) = ByteString.copyFrom(MessageDigest.getInstance("SHA-256").digest(bytes))

    private suspend fun waitFor(state: TransferState) = withTimeout(5_000) {
        engine.transfers.first { list -> list.any { it.state == state } }
    }

    @Test
    fun theStatusSaysFilesAreOffUntilTurnedOnAndFollowsEachChange() = runBlocking {
        policy.enabled = false
        engine.onPolicyChanged()
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, peer.readStart().files().availability)

        policy.enabled = true
        policy.storageGranted = false
        engine.onPolicyChanged()
        val needsStorage = io { peer.read()!! }.files()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED, needsStorage.availability)
        assertEquals("storage", needsStorage.permission)

        policy.storageGranted = true
        engine.onPolicyChanged()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, io { peer.read()!! }.files().availability)
    }

    @Test
    fun aFileFromTheDesktopIsSavedOnceItsHashMatches() = runBlocking {
        val peer = connect()
        peer.readStart()
        val data = ByteArray(300_000) { (it % 251).toByte() }
        val id = ByteString.copyFrom(ByteArray(16) { 3 })
        val accept = peer.request {
            fileOffer = fileOffer {
                transferId = id
                files += fileInfo {
                    name = "report.pdf"
                    size = data.size.toLong()
                }
            }
        }
        assertEquals(PayloadCase.FILE_ACCEPT, accept.payloadCase)
        assertEquals(0, accept.fileAccept.fileIndex)
        assertEquals(0L, accept.fileAccept.offset)

        val split = CHUNK
        val first = peer.request {
            fileChunk = fileChunk {
                transferId = id
                this.data = ByteString.copyFrom(data, 0, split)
            }
        }
        assertEquals(PayloadCase.ACK, first.payloadCase)
        val last = peer.request {
            fileChunk = fileChunk {
                transferId = id
                offset = split.toLong()
                this.data = ByteString.copyFrom(data, split, data.size - split)
                sha256 = sha256(data)
            }
        }
        assertEquals(PayloadCase.ACK, last.payloadCase)
        assertArrayEquals(data, store.saved["report.pdf"])
        waitFor(TransferState.Completed)
        Unit
    }

    @Test
    fun aSlowWriteNeverHoldsUpTheReader() = runBlocking {
        val peer = connect()
        peer.readStart()
        val id = ByteString.copyFrom(ByteArray(16) { 4 })
        val data = ByteArray(1000) { it.toByte() }
        peer.request {
            fileOffer = fileOffer {
                transferId = id
                files += fileInfo {
                    name = "slow.bin"
                    size = data.size.toLong()
                }
            }
        }
        val gate = CountDownLatch(1)
        store.gate = gate
        val chunkSeq = io {
            peer.send {
                expectsReply = true
                fileChunk = fileChunk {
                    transferId = id
                    this.data = ByteString.copyFrom(data)
                    sha256 = sha256(data)
                }
            }
        }
        // The chunk is still being written, and the phone answers a Ping all the same.
        peer.assertNothingSent()
        gate.countDown()
        val ack = io { peer.read()!! }
        assertEquals(chunkSeq, ack.replyTo)
        assertEquals(PayloadCase.ACK, ack.payloadCase)
        assertArrayEquals(data, store.saved["slow.bin"])
    }

    @Test
    fun withoutFilesInTheDesktopsHelloItsOffersAreUnsupported() = runBlocking {
        val peer = connect(features = emptyList())
        peer.readStart()
        val answer = peer.request {
            fileOffer = fileOffer {
                transferId = ByteString.copyFrom(ByteArray(16))
                files += fileInfo { name = "a" }
            }
        }
        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, answer.errorReply.code)
    }

    @Test
    fun thePhoneSendsOnceFilesAreReadyOnTheDesktop() = runBlocking {
        val data = ByteArray(CHUNK + 1000) { (it % 13).toByte() }
        val key = engine.send(peer, listOf(MemoryFile("photo.jpg", data))).single()
        val desktopPeer = connect()
        desktopPeer.readStart()
        // Nothing goes out before the desktop says files are ready there.
        desktopPeer.assertNothingSent()
        desktopPeer.sendStatus(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED)
        desktopPeer.assertNothingSent()
        desktopPeer.sendStatus(FeatureAvailability.FEATURE_AVAILABILITY_READY)

        val offer = desktopPeer.readRequest(PayloadCase.FILE_OFFER)
        assertEquals(key.id.bytes, offer.fileOffer.transferId)
        assertEquals("photo.jpg", offer.fileOffer.filesList.single().name)
        desktopPeer.reply(offer) { fileAccept = fileAccept { } }

        val chunks = List(2) { desktopPeer.readRequest(PayloadCase.FILE_CHUNK) }
        assertEquals(listOf(0L, CHUNK.toLong()), chunks.map { it.fileChunk.offset })
        assertEquals(sha256(data), chunks.last().fileChunk.sha256)
        chunks.forEach { desktopPeer.reply(it) { ack = Ack.getDefaultInstance() } }
        waitFor(TransferState.Completed)
        Unit
    }

    @Test
    fun anInterruptedTransferIsOfferedAgainOnTheNextSession() = runBlocking {
        val data = ByteArray(CHUNK + 10) { (it % 7).toByte() }
        val key = engine.send(peer, listOf(MemoryFile("video.mp4", data))).single()
        val first = connect()
        first.readStart()
        first.sendStatus(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        val offer = first.readRequest(PayloadCase.FILE_OFFER)
        first.reply(offer) { fileAccept = fileAccept { } }
        val chunk = first.readRequest(PayloadCase.FILE_CHUNK)
        first.reply(chunk) { ack = Ack.getDefaultInstance() }
        first.readRequest(PayloadCase.FILE_CHUNK)
        // The connection drops before the second chunk is answered.
        io { first.close() }
        waitFor(TransferState.Interrupted)

        val second = connect()
        second.readStart()
        second.sendStatus(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        val again = second.readRequest(PayloadCase.FILE_OFFER)
        assertEquals(key.id.bytes, again.fileOffer.transferId)
        second.reply(again) { fileAccept = fileAccept { offset = CHUNK.toLong() } }
        val rest = second.readRequest(PayloadCase.FILE_CHUNK)
        assertEquals(CHUNK.toLong(), rest.fileChunk.offset)
        assertEquals(sha256(data), rest.fileChunk.sha256)
        second.reply(rest) { ack = Ack.getDefaultInstance() }
        waitFor(TransferState.Completed)
        Unit
    }

    @Test
    fun anAnswerTheProtocolDoesNotAllowEndsTheSession() = runBlocking {
        engine.send(peer, listOf(MemoryFile("a", ByteArray(10))))
        val desktopPeer = connect()
        desktopPeer.readStart()
        desktopPeer.sendStatus(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        val offer = desktopPeer.readRequest(PayloadCase.FILE_OFFER)
        // An offer is answered by FileAccept or ErrorReply, never by Ack.
        desktopPeer.reply(offer) { ack = Ack.getDefaultInstance() }
        val goodbye = io { generateSequence { desktopPeer.read() }.first { it.hasGoodbye() } }
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, goodbye.goodbye.reason)
    }

    /** Pings the phone and checks that its `Pong` comes next: nothing was sent before it. */
    private suspend fun DesktopPeer.assertNothingSent() = io {
        send { ping = ping { nonce = 7 } }
        assertEquals(PayloadCase.PONG, read()!!.payloadCase)
    }

    private companion object {
        /** The most data a chunk carries. */
        const val CHUNK = FileEngine.MAX_CHUNK_BYTES
    }
}
