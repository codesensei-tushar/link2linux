package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.clipboard.ClipText
import dev.link2linux.android.core.clipboard.ClipboardEngine
import dev.link2linux.android.core.clipboard.ClipboardWriter
import dev.link2linux.android.core.clipboard.DesktopOutcome
import dev.link2linux.android.core.clipboard.MAX_TEXT_BYTES
import dev.link2linux.android.core.clipboard.PushOutcome
import dev.link2linux.android.core.clipboard.ShareResult
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
import dev.link2linux.android.core.notifications.HmacNotificationKeys
import dev.link2linux.android.core.notifications.ImageStore
import dev.link2linux.android.core.notifications.NotificationCanceller
import dev.link2linux.android.core.notifications.NotificationEngine
import dev.link2linux.android.core.notifications.NotificationEnvironment
import dev.link2linux.android.core.notifications.NotificationMapper
import dev.link2linux.android.core.notifications.NotificationPolicy
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.clipboardPush
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.notificationSyncRequest
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The clipboard feature over real loopback TLS sessions, combined with
 * notifications as the app combines them, with a fake clipboard behind the
 * engine.
 */
class ClipboardSessionTest {

    private class Policy : NotificationPolicy {
        @Volatile override var enabled = true

        @Volatile override var accessGranted = true
        override val dismissSync = true
        override val hideOneTimeCodes = true
        override val actWhileLocked = false

        override fun isExcluded(packageName: String) = false

        override fun isContentHidden(packageName: String) = false
    }

    private class FakeClipboard : ClipboardWriter {
        val written = CopyOnWriteArrayList<ClipText>()

        @Volatile var refusal: Exception? = null

        override suspend fun write(clip: ClipText) {
            refusal?.let { throw it }
            written += clip
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val policy = Policy()
    private val images = ImageStore()
    private val notifications = NotificationEngine(
        scope = scope,
        policy = policy,
        environment = object : NotificationEnvironment {
            override val isLocked = false
            override val canStartActivitiesFromBackground = false
        },
        mapper = NotificationMapper(images, keys = HmacNotificationKeys(ByteArray(32) { 5 })),
        images = images,
        canceller = NotificationCanceller { true },
        ownPackage = "dev.link2linux.android",
    )
    private val enabled = MutableStateFlow(true)
    private val clipboard = FakeClipboard()
    private val engine = ClipboardEngine(scope, clipboard, enabled)
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
        hooks = CompositeSessionHooks(
            NotificationSessionHooks(notifications, scope),
            ClipboardSessionHooks(engine, requestTimeoutMillis = REQUEST_TIMEOUT_MILLIS),
        ),
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

    /** Pairs [desktop] in the store if needed and connects it, listing [features] in its Hello. */
    private suspend fun connect(
        features: List<String> = listOf("notifications", "clipboard"),
        desktop: TestDesktop = this.desktop,
        deviceId: String = DesktopPeer.DEVICE_ID,
        name: String = "Test desktop",
    ): DesktopPeer {
        val key = checkNotNull(desktop.fingerprint)
        if (devices.find(key) == null) {
            devices.save(
                PairedDevice(
                    device = Device(DeviceId(deviceId), name, DeviceKind.DESKTOP),
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
                sendHello(PairingMethod.PAIRING_METHOD_UNSPECIFIED, deviceId, features, name)
                assertEquals(listOf("clipboard", "notifications"), readHello().featuresList.sorted())
            }
        }
    }

    /** Reads `FeatureStatus` and `PresenceUpdate`, and returns the states the status lists. */
    private suspend fun DesktopPeer.readStart(): Map<String, FeatureAvailability> = io {
        val status = read()!!
        assertEquals(PayloadCase.PRESENCE_UPDATE, read()!!.payloadCase)
        status.states()
    }

    private fun Envelope.states(): Map<String, FeatureAvailability> {
        assertEquals(PayloadCase.FEATURE_STATUS, payloadCase)
        return featureStatus.featuresList.associate { it.feature to it.availability }
    }

    /** Tells the phone the desktop takes pushes, and waits until [count] desktops are ready. */
    private suspend fun DesktopPeer.sendReady(count: Int = 1) {
        io {
            send {
                featureStatus = featureStatus {
                    features += featureState {
                        feature = "notifications"
                        availability = FeatureAvailability.FEATURE_AVAILABILITY_READY
                    }
                    features += featureState {
                        feature = "clipboard"
                        availability = FeatureAvailability.FEATURE_AVAILABILITY_READY
                    }
                }
            }
        }
        withTimeout(5_000) { engine.connected.first { list -> list.count { it.isReady } == count } }
    }

    private suspend fun DesktopPeer.push(text: String, sensitive: Boolean = false): Envelope = io {
        val request = send {
            expectsReply = true
            clipboardPush = clipboardPush {
                this.text = text
                this.sensitive = sensitive
            }
        }
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    /** Reads the phone's next push and checks it is a request. */
    private suspend fun DesktopPeer.readPush(): Envelope = io {
        read()!!.also {
            assertEquals(PayloadCase.CLIPBOARD_PUSH, it.payloadCase)
            assertTrue(it.expectsReply)
        }
    }

    private suspend fun DesktopPeer.ack(request: Envelope) = io {
        send {
            replyTo = request.seq
            ack = Ack.getDefaultInstance()
        }
    }

    /** Pings the phone and checks that its `Pong` comes next: nothing was sent before it. */
    private suspend fun DesktopPeer.assertNothingSent() = io {
        send { ping = ping { nonce = 7 } }
        assertEquals(PayloadCase.PONG, read()!!.payloadCase)
    }

    private fun shareAsync(text: String) = scope.async(start = CoroutineStart.UNDISPATCHED) { engine.share(ClipText(text)) }

    private fun sent(vararg names: String) = ShareResult.Pushed(names.map { DesktopOutcome(it, PushOutcome.SENT) })

    @Test
    fun theStatusHoldsEveryFeatureAndTheClipboardIsOffUntilTurnedOn() = runBlocking {
        enabled.value = false
        val peer = connect()

        val states = peer.readStart()

        assertEquals(setOf("notifications", "clipboard"), states.keys)
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, states["clipboard"])
    }

    @Test
    fun eachChangeSendsTheCompleteStatus() = runBlocking {
        enabled.value = false
        val peer = connect()
        val atStart = peer.readStart()

        enabled.value = true
        val on = io { peer.read()!! }.states()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, on["clipboard"])
        assertEquals(atStart["notifications"], on["notifications"])

        // The other feature's change carries the clipboard's state too.
        policy.enabled = false
        notifications.onPolicyChanged()
        val notificationsOff = io { peer.read()!! }.states()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, notificationsOff["notifications"])
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, notificationsOff["clipboard"])
    }

    @Test
    fun aPushGoesOnTheClipboardAndIsAcknowledged() = runBlocking {
        val peer = connect()
        peer.readStart()

        assertEquals(PayloadCase.ACK, peer.push("secret", sensitive = true).payloadCase)

        assertEquals(listOf(ClipText("secret", sensitive = true)), clipboard.written.toList())
    }

    @Test
    fun aPushWhileTheFeatureIsOffIsRefused() = runBlocking {
        enabled.value = false
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, peer.push("hi").errorReply.code)
        assertTrue(clipboard.written.isEmpty())
    }

    @Test
    fun theLimitIsTakenAndTextOverItRefused() = runBlocking {
        val peer = connect()
        peer.readStart()
        val longest = "é".repeat(MAX_TEXT_BYTES / 2)

        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, peer.push(longest + "a").errorReply.code)
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, peer.push("").errorReply.code)
        assertEquals(PayloadCase.ACK, peer.push(longest).payloadCase)
        assertEquals(longest, clipboard.written.single().text)
    }

    @Test
    fun aPushTheClipboardRefusesGetsUnavailable() = runBlocking {
        clipboard.refusal = IllegalStateException("No clipboard")
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, peer.push("hi").errorReply.code)
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupportedAndNoShares() = runBlocking {
        val peer = connect(features = listOf("notifications"))
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, peer.push("hi").errorReply.code)
        assertEquals(ShareResult.NoDesktop, engine.share(ClipText("hi")))
        assertTrue(clipboard.written.isEmpty())
    }

    @Test
    fun aShareReachesAReadyDesktop() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.sendReady()

        val result = scope.async { engine.share(ClipText("from the phone", sensitive = true)) }
        val request = peer.readPush()
        assertEquals("from the phone", request.clipboardPush.text)
        assertTrue(request.clipboardPush.sensitive)
        peer.ack(request)

        assertEquals(sent("Test desktop"), withTimeout(5_000) { result.await() })
    }

    @Test
    fun nothingIsSharedBeforeTheDesktopSaysItIsReady() = runBlocking {
        val peer = connect()
        peer.readStart()
        withTimeout(5_000) { engine.connected.first { it.isNotEmpty() } }

        val result = engine.share(ClipText("hi"))

        assertEquals(ShareResult.Pushed(listOf(DesktopOutcome("Test desktop", PushOutcome.FAILED))), result)
        peer.assertNothingSent()
    }

    @Test
    fun aDesktopWithTheFeatureOffTurnsTheShareDown() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.sendReady()

        val result = scope.async { engine.share(ClipText("hi")) }
        val request = peer.readPush()
        io {
            peer.send {
                replyTo = request.seq
                errorReply = errorReply { code = ErrorCode.ERROR_CODE_DISABLED }
            }
        }

        val outcome = withTimeout(5_000) { result.await() }
        assertEquals(ShareResult.Pushed(listOf(DesktopOutcome("Test desktop", PushOutcome.TURNED_OFF))), outcome)
    }

    @Test
    fun anAnswerIsMatchedByReplyToAndALateOneIsIgnored() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.sendReady()

        // No answer in time: the push fails.
        val unanswered = scope.async { engine.share(ClipText("first")) }
        val first = peer.readPush()
        val failed = withTimeout(5_000) { unanswered.await() }
        assertEquals(ShareResult.Pushed(listOf(DesktopOutcome("Test desktop", PushOutcome.FAILED))), failed)

        // Its late answer, and one that names nothing, change nothing.
        val answered = scope.async { engine.share(ClipText("second")) }
        val second = peer.readPush()
        peer.ack(first)
        io {
            peer.send {
                replyTo = second.seq + 50
                ack = Ack.getDefaultInstance()
            }
        }
        peer.assertNothingSent()
        assertFalse(answered.isCompleted)

        peer.ack(second)
        assertEquals(sent("Test desktop"), withTimeout(5_000) { answered.await() })
    }

    @Test
    fun onePushWaitsAndOnlyTheNewestQueuedShareFollows() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.sendReady()

        val first = shareAsync("1")
        val firstPush = peer.readPush()
        val second = shareAsync("2")
        val third = shareAsync("3")
        assertEquals(
            ShareResult.Pushed(listOf(DesktopOutcome("Test desktop", PushOutcome.REPLACED))),
            withTimeout(5_000) { second.await() },
        )

        peer.ack(firstPush)
        val next = peer.readPush()
        assertEquals("3", next.clipboardPush.text)
        peer.ack(next)

        assertEquals(sent("Test desktop"), withTimeout(5_000) { first.await() })
        assertEquals(sent("Test desktop"), withTimeout(5_000) { third.await() })
        peer.assertNothingSent()
    }

    @Test
    fun notificationRequestsStillReachTheirFeature() = runBlocking {
        notifications.onListenerConnected(emptyList())
        val peer = connect()
        peer.readStart()

        val reply = io {
            val request = peer.send {
                expectsReply = true
                notificationSyncRequest = notificationSyncRequest { }
            }
            peer.read()!!.also { assertEquals(request, it.replyTo) }
        }

        assertEquals(PayloadCase.NOTIFICATION_SYNC_BATCH, reply.payloadCase)
    }

    @Test
    fun aShareGoesToEachDesktopAndALostOneFails() = runBlocking {
        val first = connect()
        first.readStart()
        first.sendReady(count = 1)
        val second = connect(desktop = TestDesktop(), deviceId = "second-desktop", name = "Other desktop")
        second.readStart()
        second.sendReady(count = 2)

        val result = scope.async { engine.share(ClipText("both")) }
        first.ack(first.readPush())
        second.readPush()
        io { second.close() }

        val outcome = withTimeout(5_000) { result.await() }
        assertEquals(
            ShareResult.Pushed(
                listOf(DesktopOutcome("Test desktop", PushOutcome.SENT), DesktopOutcome("Other desktop", PushOutcome.FAILED)),
            ),
            outcome,
        )
    }

    private companion object {
        /** Short, so the timeout test does not take the protocol's 30 seconds. */
        const val REQUEST_TIMEOUT_MILLIS = 2_000L
    }
}
