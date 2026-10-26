package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.discovery.android.NsdAdvertiser
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.QrPairingOffer
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress

@OptIn(ExperimentalCoroutinesApi::class)
class DiscoveryControllerTest {

    private class FakePairing : PairingController {
        override val state = MutableStateFlow<PairingState>(PairingState.Idle)
        override fun startQrPairing(offer: QrPairingOffer) = Unit
        override fun startSasPairing() = Unit
        override fun confirmCode(matches: Boolean) = Unit
        override fun confirmReplace(replace: Boolean) = Unit
        override fun cancel() = Unit
        override fun dismiss() = Unit
        override suspend fun unpair(fingerprint: KeyFingerprint) = Unit
    }

    private class FakeConnections : DesktopConnections {
        override val listeningPort = MutableStateFlow<Int?>(null)
        override val connected = MutableStateFlow<Set<KeyFingerprint>>(emptySet())
    }

    private class FakeNetworks : LocalNetworks {
        override val networks = MutableStateFlow<List<LocalNetwork>>(emptyList())
    }

    private class FakeAdvertiser : ServiceAdvertiser {
        val registrations = mutableListOf<Pair<Announcement, Int>>()
        var unregistered = 0
        override val registeredName: StateFlow<String?> = MutableStateFlow(null)
        override suspend fun register(announcement: Announcement, port: Int) {
            registrations += announcement to port
        }
        override suspend fun unregister() {
            unregistered++
        }
    }

    private class FakeSender : BeaconSender {
        val sent = mutableListOf<Pair<ByteArray, List<InetSocketAddress>>>()
        override suspend fun send(networks: List<LocalNetwork>, datagram: ByteArray, unicast: List<InetSocketAddress>) {
            sent += datagram to unicast
        }
    }

    private class FakeLocks : TimedLock, HeldLock {
        var multicast = false
        var holds = 0
        override fun hold(millis: Long) {
            holds++
        }
        override fun acquire() {
            multicast = true
        }
        override fun release() {
            multicast = false
        }
    }

    private val pairing = FakePairing()
    private val connections = FakeConnections()
    private val networks = FakeNetworks()
    private val advertiser = FakeAdvertiser()
    private val sender = FakeSender()
    private val locks = FakeLocks()
    private val repository = InMemoryPairedDeviceRepository()
    private val wifi = LocalNetwork(1, listOf(NetworkAddress(InetAddress.getByName("192.168.1.5"), 24)))
    private val key = PresenceKey.of(ByteArray(32) { 5 })
    private val desktop = PairedDevice(
        Device(DeviceId("d"), "Desk", DeviceKind.DESKTOP),
        KeyFingerprint.of(ByteArray(32) { 3 }),
        pairedAtEpochMillis = 0,
        presenceKey = key,
        lastKnownAddresses = listOf(InetSocketAddress(InetAddress.getByName("192.168.1.20"), 27381)),
    )

    private fun TestScope.controller(): DiscoveryController {
        var n = 0
        val announcer = Announcer { size -> ByteArray(size) { (n++).toByte() } }
        return DiscoveryController(
            backgroundScope, repository, pairing, connections, networks, advertiser, sender, locks, locks,
            announcer, wallClockMillis = { 1_000_000 + currentTime },
        ).also { it.start() }
    }

    private fun up() {
        connections.listeningPort.value = 27380
        networks.networks.value = listOf(wifi)
    }

    @Test
    fun announcesNothingWithoutAPortAndANetwork() = runTest {
        val controller = controller()
        runCurrent()
        assertTrue(advertiser.registrations.isEmpty())
        connections.listeningPort.value = 27380
        runCurrent()
        assertTrue(advertiser.registrations.isEmpty())
        assertNull(controller.announced.value)
    }

    @Test
    fun unpairedPhoneRegistersWithTheVersionOnly() = runTest {
        controller()
        up()
        runCurrent()
        val (announcement, port) = advertiser.registrations.last()
        assertEquals(27380, port)
        assertEquals(mapOf("v" to "1"), announcement.txt)
        assertTrue(Regex("l2l-[0-9a-f]{8}").matches(announcement.instanceName))
        assertTrue(sender.sent.isEmpty()) // nothing paired, nothing scanned: no beacons
    }

    @Test
    fun pairedPhoneAnnouncesAPresenceTagAndStartsABurst() = runTest {
        controller()
        repository.save(desktop)
        up()
        runCurrent()
        val txt = advertiser.registrations.last().first.txt
        assertEquals(setOf("v", "n", "t"), txt.keys)
        assertEquals(1, sender.sent.size)
        assertEquals(listOf(desktop.lastKnownAddresses), sender.sent.map { it.second })
        assertEquals(Beacon.SIZE, sender.sent.single().first.size)
        // The burst re-registered the service to make Android announce again.
        assertTrue(advertiser.registrations.size >= 2)
    }

    @Test
    fun theSecretOfAScannedCodeAddsThePairingTagAndHoldsTheMulticastLock() = runTest {
        controller()
        up()
        val offer = QrPairingOffer(desktop.fingerprint, PairingSecret.of(ByteArray(16)), "Desk")
        pairing.state.value = PairingState.AwaitingDesktop.Qr(offer, untilEpochMillis = 1_000_000 + 300_000)
        runCurrent()
        assertEquals(Tags.pairing(offer.secret).toBase64Url(), advertiser.registrations.last().first.txt["q"])
        assertTrue(locks.multicast)
        assertEquals(1, sender.sent.size)
        assertEquals(1, sender.sent.single().first[5].toInt()) // pairing beacon

        pairing.state.value = PairingState.Idle
        runCurrent()
        assertTrue(!locks.multicast)
        assertNull(advertiser.registrations.last().first.txt["q"])
    }

    @Test
    fun sasModeAnnouncesPmAndKeepsItsName() = runTest {
        controller()
        up()
        pairing.state.value = PairingState.AwaitingDesktop.Sas(untilEpochMillis = 1_000_000 + 120_000, attemptsLeft = 3)
        runCurrent()
        val first = advertiser.registrations.last().first
        assertEquals("1", first.txt["pm"])
        networks.networks.value = listOf(wifi, LocalNetwork(2, emptyList()))
        advanceTimeBy(16 * 60_000)
        assertEquals(first.instanceName, advertiser.registrations.last().first.instanceName)
    }

    @Test
    fun joiningANetworkAndTheRotationTimerChangeTheNameAndNonce() = runTest {
        controller()
        repository.save(desktop)
        up()
        runCurrent()
        val first = advertiser.registrations.last().first
        networks.networks.value = listOf(wifi, LocalNetwork(2, emptyList()))
        runCurrent()
        val second = advertiser.registrations.last().first
        assertTrue(first.instanceName != second.instanceName)
        assertTrue(first.txt["n"] != second.txt["n"])

        advanceTimeBy(Announcer.ROTATE_MILLIS + 1)
        runCurrent()
        assertTrue(second.instanceName != advertiser.registrations.last().first.instanceName)
    }

    @Test
    fun leavingAllNetworksUnregistersAndStopsBeacons() = runTest {
        val controller = controller()
        repository.save(desktop)
        up()
        runCurrent()
        networks.networks.value = emptyList()
        runCurrent()
        assertEquals(1, advertiser.unregistered)
        assertNull(controller.announced.value)
        val beacons = sender.sent.size
        advanceTimeBy(60_000)
        assertEquals(beacons, sender.sent.size)
    }

    @Test
    fun theMostRecentlyConnectedDesktopGetsTheFirstTag() = runTest {
        controller()
        val other = desktop.copy(
            fingerprint = KeyFingerprint.of(ByteArray(32) { 4 }),
            presenceKey = PresenceKey.of(ByteArray(32) { 6 }),
            lastConnectedEpochMillis = 500,
        )
        repository.save(desktop.copy(lastConnectedEpochMillis = 100))
        repository.save(other)
        up()
        runCurrent()
        val txt = advertiser.registrations.last().first.txt
        assertEquals(setOf("v", "n", "t", "t2"), txt.keys)
        val nonce = java.util.Base64.getUrlDecoder().decode(txt.getValue("n"))
        assertEquals(Tags.presence(other.presenceKey, nonce).toBase64Url(), txt["t"])
    }

    @Test
    fun legacyNamesAreUnescaped() {
        assertEquals("l2l-ab12 (2)", NsdAdvertiser.unescapeName("""l2l-ab12\032(2)"""))
    }
}
