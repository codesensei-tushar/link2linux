package dev.link2linux.android.core.network.features

import com.google.protobuf.ByteString
import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
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
import dev.link2linux.android.core.notifications.AndroidValues
import dev.link2linux.android.core.notifications.ImageStore
import dev.link2linux.android.core.notifications.NotificationCanceller
import dev.link2linux.android.core.notifications.NotificationEngine
import dev.link2linux.android.core.notifications.NotificationEnvironment
import dev.link2linux.android.core.notifications.NotificationMapper
import dev.link2linux.android.core.notifications.NotificationPolicy
import dev.link2linux.android.core.notifications.NotificationSnapshot
import dev.link2linux.android.core.notifications.RemovalCause
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.Goodbye
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.notificationDismissRequest
import dev.link2linux.protocol.v1.notificationSyncRequest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

/** The notifications feature over real loopback TLS sessions, with a fake platform behind the engine. */
class NotificationSessionTest {

    private class Policy : NotificationPolicy {
        @Volatile override var enabled = true

        @Volatile override var accessGranted = true
        override val dismissSync = true

        override fun isExcluded(packageName: String) = false

        override fun isContentHidden(packageName: String) = false
    }

    private val policy = Policy()
    private val cancelled = java.util.concurrent.CopyOnWriteArrayList<String>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val images = ImageStore()
    private val engine = NotificationEngine(
        scope = scope,
        policy = policy,
        environment = object : NotificationEnvironment {
            override val isLocked = false
            override val canStartActivitiesFromBackground = false
        },
        mapper = NotificationMapper(images),
        images = images,
        canceller = NotificationCanceller { key ->
            cancelled += key
            true
        },
        ownPackage = "dev.link2linux.android",
        onError = { it.printStackTrace() },
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
        hooks = NotificationSessionHooks(engine, scope),
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
        features: List<String> = listOf("notifications"),
        desktop: TestDesktop = this.desktop,
        deviceId: String = DesktopPeer.DEVICE_ID,
    ): DesktopPeer {
        if (devices.find(desktop.fingerprint!!) == null) {
            devices.save(
                PairedDevice(
                    device = Device(DeviceId(deviceId), "Test desktop", DeviceKind.DESKTOP),
                    fingerprint = desktop.fingerprint!!,
                    pairedAtEpochMillis = 0,
                    presenceKey = PresenceKey.of(ByteArray(32) { 9 }),
                ),
            )
        }
        service.start()
        val port = withTimeout(5_000) { service.listeningPort.filterNotNull().first() }
        return io {
            DesktopPeer(desktop.connect(port), desktop).apply {
                sendHello(PairingMethod.PAIRING_METHOD_UNSPECIFIED, deviceId, features)
                assertTrue("notifications" in readHello().featuresList)
            }
        }
    }

    /** Reads FeatureStatus and PresenceUpdate and returns the status' availability. */
    private suspend fun DesktopPeer.readStart(): FeatureAvailability = io {
        val status = read()!!
        assertEquals(PayloadCase.FEATURE_STATUS, status.payloadCase)
        assertEquals(PayloadCase.PRESENCE_UPDATE, read()!!.payloadCase)
        status.availability()
    }

    private fun Envelope.availability() = featureStatus.featuresList.single().availability

    /** Sends a sync request and returns every batch of the answer. */
    private suspend fun DesktopPeer.sync(): List<Envelope> = io {
        val request = send {
            expectsReply = true
            notificationSyncRequest = notificationSyncRequest { }
        }
        buildList {
            do {
                val reply = read()!!
                assertEquals(request, reply.replyTo)
                add(reply)
            } while (reply.notificationSyncBatch.more)
        }
    }

    private fun keys(batches: List<Envelope>) = batches.flatMap { it.notificationSyncBatch.notificationsList }.map { it.key }

    private fun snap(key: String) = NotificationSnapshot(
        key = key,
        packageName = "org.chat",
        postTimeMillis = 5,
        title = "T",
        text = "hello",
        importance = AndroidValues.IMPORTANCE_HIGH,
    )

    /** The platform's events reach the engine through a queue; wait until a sync shows them. */
    private suspend fun DesktopPeer.syncUntil(expected: List<String>): List<Envelope> {
        var batches = sync()
        withTimeout(5_000) {
            while (keys(batches) != expected) {
                delay(20)
                batches = sync()
            }
        }
        return batches
    }

    @Test
    fun theSessionStartsWithTheEnginesAvailability() = runBlocking {
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())
        val status = service.connected.first { it.isNotEmpty() }
        assertEquals(setOf(desktop.fingerprint), status)
    }

    @Test
    fun aSyncRequestGetsBatchesThatAnswerIt() = runBlocking {
        engine.onListenerConnected(listOf(snap("a"), snap("b")))
        val peer = connect()
        peer.readStart()

        val batches = peer.syncUntil(listOf("a", "b"))

        assertTrue(batches.first().notificationSyncBatch.reset)
        assertFalse(batches.last().notificationSyncBatch.more)
    }

    @Test
    fun aNotificationPostedAfterTheSyncArrivesAsALiveEvent() = runBlocking {
        engine.onListenerConnected(emptyList())
        val peer = connect()
        peer.readStart()
        peer.syncUntil(emptyList())

        engine.onPosted(snap("live"))
        val event = io { peer.read()!! }

        assertEquals(PayloadCase.NOTIFICATION_POSTED, event.payloadCase)
        assertEquals("live", event.notificationPosted.notification.key)
        assertEquals(0L, event.replyTo)
        assertFalse(event.expectsReply)

        engine.onRemoved("live", RemovalCause.USER_DISMISSED)
        val removed = io { peer.read()!! }
        assertEquals(PayloadCase.NOTIFICATION_REMOVED, removed.payloadCase)
        assertEquals("live", removed.notificationRemoved.key)
    }

    @Test
    fun aDismissRequestCancelsTheNotificationAndIsAcknowledged() = runBlocking {
        engine.onListenerConnected(listOf(snap("n1")))
        val peer = connect()
        peer.readStart()
        peer.syncUntil(listOf("n1"))

        val request = io {
            peer.send {
                expectsReply = true
                notificationDismissRequest = notificationDismissRequest {
                    idempotencyKey = ByteString.copyFrom(ByteArray(16) { 3 })
                    createdUnixMs = System.currentTimeMillis()
                    key = "n1"
                }
            }
        }
        val reply = io { peer.read()!! }

        assertEquals(PayloadCase.ACK, reply.payloadCase)
        assertEquals(request, reply.replyTo)
        assertEquals(listOf("n1"), cancelled.toList())
    }

    @Test
    fun aRequestWhileTheFeatureIsNotReadyGetsTheEnginesError() = runBlocking {
        policy.enabled = false
        engine.onPolicyChanged()
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, peer.readStart())

        val request = io {
            peer.send {
                expectsReply = true
                notificationSyncRequest = notificationSyncRequest { }
            }
        }
        val reply = io { peer.read()!! }

        assertEquals(request, reply.replyTo)
        assertEquals(ErrorCode.ERROR_CODE_DISABLED, reply.errorReply.code)
    }

    @Test
    fun featureStatusFollowsTheEnginesAvailability() = runBlocking {
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())

        policy.accessGranted = false
        engine.onPolicyChanged()
        val missing = io { peer.read()!! }
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED, missing.availability())
        assertEquals("notification-access", missing.featureStatus.featuresList.single().permission)

        policy.accessGranted = true
        engine.onPolicyChanged()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, io { peer.read()!! }.availability())
    }

    @Test
    fun aNewConnectionFromTheSameDesktopReplacesTheOldOne() = runBlocking {
        engine.onListenerConnected(emptyList())
        val old = connect()
        old.readStart()
        val replacement = connect()
        replacement.readStart()

        val goodbye = io { old.read()!! }
        assertEquals(GoodbyeReason.GOODBYE_REASON_REPLACED, goodbye.goodbye.reason)
        assertNull(io { old.read() })

        // The old session ending must not detach the new one.
        replacement.syncUntil(emptyList())
        engine.onPosted(snap("after"))
        assertEquals("after", io { replacement.read()!! }.notificationPosted.notification.key)
        assertEquals(1, service.connected.value.size)
    }

    @Test
    fun twoConnectedDesktopsBothGetLiveEvents() = runBlocking {
        engine.onListenerConnected(emptyList())
        val first = connect()
        first.readStart()
        val second = connect(desktop = TestDesktop(), deviceId = "second-desktop")
        second.readStart()
        first.syncUntil(emptyList())
        second.syncUntil(emptyList())

        engine.onPosted(snap("both"))
        assertEquals("both", io { first.read()!! }.notificationPosted.notification.key)
        assertEquals("both", io { second.read()!! }.notificationPosted.notification.key)

        // One leaving does not take the other's events with it.
        io { first.send { goodbye = Goodbye.newBuilder().setReason(GoodbyeReason.GOODBYE_REASON_SHUTDOWN).build() } }
        withTimeout(5_000) { service.connected.first { it.size == 1 } }
        engine.onPosted(snap("after"))
        assertEquals("after", io { second.read()!! }.notificationPosted.notification.key)
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupported() = runBlocking {
        val peer = connect(features = emptyList())
        peer.readStart()

        val request = io {
            peer.send {
                expectsReply = true
                notificationSyncRequest = notificationSyncRequest { }
            }
        }
        val reply = io { peer.read()!! }

        assertEquals(request, reply.replyTo)
        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, reply.errorReply.code)
    }
}
