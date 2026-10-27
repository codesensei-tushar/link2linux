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
import dev.link2linux.android.core.status.StatusEngine
import dev.link2linux.android.core.status.StatusSource
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Battery
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.PhoneStatus
import dev.link2linux.protocol.v1.PhoneStatusRequest
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Test
import java.net.InetAddress

/**
 * The phone status feature over real loopback TLS sessions, with a fake
 * platform behind the engine. The engine's own tests cover its pacing; here
 * it sends updates 100 ms apart.
 */
class StatusSessionTest {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val platform = MutableStateFlow(battery(80))
    private val enabled = MutableStateFlow(true)
    private val engine = StatusEngine(
        scope = scope,
        source = StatusSource { platform },
        enabled = enabled,
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
        hooks = StatusSessionHooks(engine, scope),
        port = 0,
        bindAddress = InetAddress.getLoopbackAddress(),
    )
    private val desktop = TestDesktop()

    @After
    fun tearDown() {
        runBlocking { service.stop() }
        scope.cancel()
    }

    private fun battery(percent: Int): PhoneStatus =
        PhoneStatus.newBuilder().setBattery(Battery.newBuilder().setPercent(percent)).build()

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    /** Pairs the desktop in the store if needed and connects it, listing [features] in its Hello. */
    private suspend fun connect(features: List<String> = listOf("status")): DesktopPeer {
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
                assertEquals(listOf("status"), readHello().featuresList)
            }
        }
    }

    /** Reads `FeatureStatus` and `PresenceUpdate`, and returns what the status says of the feature. */
    private suspend fun DesktopPeer.readStart(): FeatureAvailability = io {
        val status = read()!!
        assertEquals(PayloadCase.PRESENCE_UPDATE, read()!!.payloadCase)
        status.availability()
    }

    private fun Envelope.availability(): FeatureAvailability {
        assertEquals(PayloadCase.FEATURE_STATUS, payloadCase)
        return featureStatus.featuresList.single { it.feature == "status" }.availability
    }

    private suspend fun DesktopPeer.sendAvailability(availability: FeatureAvailability) = io {
        send {
            featureStatus = featureStatus {
                features += featureState {
                    feature = "status"
                    this.availability = availability
                }
            }
        }
    }

    /** Asks for the status and returns the answer. */
    private suspend fun DesktopPeer.request(): Envelope = io {
        val request = send {
            expectsReply = true
            phoneStatusRequest = PhoneStatusRequest.getDefaultInstance()
        }
        read()!!.also { assertEquals(request, it.replyTo) }
    }

    private suspend fun DesktopPeer.readChanged(): PhoneStatus = io {
        read()!!.also { assertEquals(PayloadCase.PHONE_STATUS_CHANGED, it.payloadCase) }.phoneStatusChanged.status
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
    fun theFeatureIsReadyUntilTheUserTurnsItOff() = runBlocking {
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())

        enabled.value = false

        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, io { peer.read()!! }.availability())
    }

    @Test
    fun aRequestIsAnsweredWithTheStatusAndChangesFollow() = runBlocking {
        val peer = connect()
        peer.readStart()

        val reply = peer.request()
        assertEquals(PayloadCase.PHONE_STATUS_REPLY, reply.payloadCase)
        assertEquals(battery(80), reply.phoneStatusReply.status)

        platform.value = battery(79)
        assertEquals(battery(79), peer.readChanged())
        platform.value = battery(78)
        assertEquals(battery(78), peer.readChanged())
    }

    @Test
    fun aRequestWhileTheFeatureIsOffIsRefused() = runBlocking {
        enabled.value = false
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, peer.readStart())

        val reply = peer.request()

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, reply.errorReply.code)
        platform.value = battery(79)
        peer.assertNothingSent()
    }

    @Test
    fun turningTheFeatureOffEndsTheUpdates() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.request()

        enabled.value = false
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, io { peer.read()!! }.availability())
        platform.value = battery(79)

        peer.assertNothingSent()
    }

    @Test
    fun theUpdatesEndWhenTheDesktopIsNoLongerReady() = runBlocking {
        val peer = connect()
        peer.readStart()
        peer.sendAvailability(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        peer.request()

        peer.sendAvailability(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED)
        // The phone handles the desktop's messages in order, so the ping orders this check after the status.
        peer.assertNothingSent()
        platform.value = battery(79)
        peer.assertNothingSent()

        // Ready again: the desktop asks again, and the updates resume.
        peer.sendAvailability(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        assertEquals(battery(79), peer.request().phoneStatusReply.status)
        platform.value = battery(78)
        assertEquals(battery(78), peer.readChanged())
    }

    @Test
    fun aRequestSentAsAnEventIsIgnored() = runBlocking {
        val peer = connect()
        peer.readStart()

        io { peer.send { phoneStatusRequest = PhoneStatusRequest.getDefaultInstance() } }
        platform.value = battery(79)

        peer.assertNothingSent()
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupported() = runBlocking {
        val peer = connect(features = listOf("notifications"))
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, peer.request().errorReply.code)
    }

    private companion object {
        const val INTERVAL_MILLIS = 100L
    }
}
