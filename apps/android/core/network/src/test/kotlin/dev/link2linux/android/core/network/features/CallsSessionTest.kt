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
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.bluetoothPairingRequest
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.net.InetAddress
import java.util.concurrent.CopyOnWriteArrayList

/** The calls feature's pairing help over real loopback TLS sessions, with a fake prompt. */
class CallsSessionTest {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val availability = MutableStateFlow(FeatureAvailability.FEATURE_AVAILABILITY_READY)
    private val prompts = CopyOnWriteArrayList<Pair<String, String>>()

    /** Whether the fake prompt can be shown, as with the app's notifications allowed. */
    @Volatile private var canPrompt = true
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
        hooks = CallsSessionHooks(availability) { name, address -> canPrompt.also { if (it) prompts += name to address } },
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
    private suspend fun connect(features: List<String> = listOf("calls")): DesktopPeer {
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
                assertEquals(listOf("calls"), readHello().featuresList)
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
        return featureStatus.featuresList.single { it.feature == "calls" }.availability
    }

    private suspend fun DesktopPeer.pair(name: String = "Laptop", address: String = "00:1a:7D:DA:71:13", request: Boolean = true) = io {
        val seq = send {
            expectsReply = request
            bluetoothPairingRequest = bluetoothPairingRequest {
                this.name = name
                this.address = address
            }
        }
        if (request) read()!!.also { assertEquals(seq, it.replyTo) } else null
    }

    @Test
    fun theFeatureFollowsItsAvailability() = runBlocking {
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, peer.readStart())

        availability.value = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED

        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_DISABLED, io { peer.read()!! }.availability())
    }

    @Test
    fun aValidRequestPromptsTheUserAndIsAcknowledged() = runBlocking {
        val peer = connect()
        peer.readStart()

        assertEquals(PayloadCase.ACK, peer.pair()?.payloadCase)

        assertEquals(listOf("Laptop" to "00:1A:7D:DA:71:13"), prompts.toList())
    }

    @Test
    fun aPromptThatCannotBeShownIsAnsweredNotAllowed() = runBlocking {
        val peer = connect()
        peer.readStart()
        canPrompt = false

        val reply = peer.pair()!!

        assertEquals(PayloadCase.ERROR_REPLY, reply.payloadCase)
        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, reply.errorReply.code)
        assertEquals(emptyList<Pair<String, String>>(), prompts.toList())
    }

    @Test
    fun aNameOrAddressOutsideTheSchemaIsRefused() = runBlocking {
        val peer = connect()
        peer.readStart()

        for ((name, address) in listOf(
            "" to "00:1A:7D:DA:71:13",
            "x".repeat(249) to "00:1A:7D:DA:71:13",
            "Laptop" to "00:1A:7D:DA:71",
            "Laptop" to "00-1A-7D-DA-71-13",
            "Laptop" to "00:1A:7D:DA:71:1G",
            "Laptop" to "00:1A:7D:DA:71:13 ",
            "Laptop" to "",
        )) {
            assertEquals("$name / $address", ErrorCode.ERROR_CODE_INVALID_ARGUMENT, peer.pair(name, address)?.errorReply?.code)
        }
        assertEquals(emptyList<Pair<String, String>>(), prompts.toList())
    }

    @Test
    fun aNameIsMeasuredInBytes() {
        fun refused(name: String) = CallsSessionHooks.pairingRefusal(
            bluetoothPairingRequest {
                this.name = name
                address = "00:1A:7D:DA:71:13"
            },
        )

        assertNull(refused("x".repeat(248)))
        // 124 characters of two bytes each fit; one more does not.
        assertNull(refused("\u00e9".repeat(124)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, refused("\u00e9".repeat(125))?.first)
    }

    @Test
    fun aPhoneThatCannotTakeCallsAnswersUnavailable() = runBlocking {
        availability.value = FeatureAvailability.FEATURE_AVAILABILITY_UNAVAILABLE
        val peer = connect()
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_UNAVAILABLE, peer.readStart())

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, peer.pair()?.errorReply?.code)
        assertEquals(emptyList<Pair<String, String>>(), prompts.toList())
    }

    @Test
    fun whileTheFeatureIsOffTheRequestIsDisabled() = runBlocking {
        availability.value = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
        val peer = connect()
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_DISABLED, peer.pair()?.errorReply?.code)
        assertEquals(emptyList<Pair<String, String>>(), prompts.toList())
    }

    @Test
    fun aRequestSentAsAnEventIsIgnored() = runBlocking {
        val peer = connect()
        peer.readStart()

        peer.pair(request = false)
        io {
            peer.send { ping = ping { nonce = 7 } }
            assertEquals(PayloadCase.PONG, peer.read()!!.payloadCase)
        }

        assertEquals(emptyList<Pair<String, String>>(), prompts.toList())
    }

    @Test
    fun aDesktopWithoutTheFeatureGetsUnsupported() = runBlocking {
        val peer = connect(features = listOf("notifications"))
        peer.readStart()

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, peer.pair()?.errorReply?.code)
        assertEquals(emptyList<Pair<String, String>>(), prompts.toList())
    }
}
