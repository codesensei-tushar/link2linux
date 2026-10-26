package dev.link2linux.android.feature.pairing

import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.discovery.ConnectionSwitch
import dev.link2linux.android.core.discovery.DiscoveryStatus
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingMethod
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.QrPairingOffer
import dev.link2linux.android.core.model.SasCode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress

@OptIn(ExperimentalCoroutinesApi::class)
class PairingViewModelTest {

    private val repository = FakePairedDeviceRepository()
    private val controller = FakeController()
    private val discovery = FakeDiscovery()
    private val connection = FakeConnection()
    private val connections = FakeConnections()

    @Before
    fun setMain() = Dispatchers.setMain(UnconfinedTestDispatcher())

    @After
    fun resetMain() = Dispatchers.resetMain()

    private val validCode = "link2linux://pair?v=1&f=_cRaTr6EOO-nZGngbLdW_xKpw0VczVObf2B7Hbdj-bU" +
        "&s=AAECAwQFBgcICQoLDA0ODw&n=B%C3%BCro%20workstation&a=192.168.1.20"

    private val desk = PairedDevice(
        Device(DeviceId("desk"), "Desk", DeviceKind.DESKTOP),
        KeyFingerprint.of(ByteArray(32)),
        pairedAtEpochMillis = 0,
        presenceKey = PresenceKey.of(ByteArray(32)),
    )

    private fun TestScope.viewModel(): PairingViewModel {
        val vm = PairingViewModel(
            repository, controller, discovery, connection, connections, { 1_000_000 + currentTime },
            flow { while (true) { emit(Unit); delay(1_000) } },
        )
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.uiState.collect {} }
        return vm
    }

    @Test
    fun listsPairedComputersOnceLoaded() = runTest {
        val vm = viewModel()
        assertEquals(emptyList<PairedDevice>(), vm.uiState.value.devices)
        repository.save(desk)
        assertEquals(listOf(desk), vm.uiState.value.devices)
        assertEquals(PairingScreenState.Home, vm.uiState.value.screen)
    }

    @Test
    fun scanningShowsTheScannerUntilACodeIsRead() = runTest {
        val vm = viewModel()
        vm.startScan()
        assertEquals(PairingScreenState.Scanning, vm.uiState.value.screen)
        vm.onQrScanned(validCode)
        val offer = controller.qrOffers.single()
        assertEquals("Büro workstation", offer.desktopName)
        assertEquals(listOf(InetSocketAddress(InetAddress.getByName("192.168.1.20"), 27381)), offer.beaconAddresses)
        assertNull(vm.uiState.value.scanError)
    }

    @Test
    fun rejectedCodesShowAnErrorAndStartNothing() = runTest {
        val vm = viewModel()
        vm.startScan()
        vm.onQrScanned("https://example.com")
        assertEquals(ScanError.NOT_A_PAIRING_CODE, vm.uiState.value.scanError)
        vm.onQrScanned("link2linux://pair?v=2&x=y")
        assertEquals(ScanError.UPDATE_THE_APP, vm.uiState.value.scanError)
        assertEquals(PairingScreenState.Scanning, vm.uiState.value.screen)
        assertEquals(0, controller.qrOffers.size)
        vm.clearScanError()
        assertNull(vm.uiState.value.scanError)
    }

    @Test
    fun aCodeReadAfterCancelStartsNothing() = runTest {
        val vm = viewModel()
        vm.startScan()
        vm.cancel()
        // A frame the scanner read just before it closed.
        vm.onQrScanned(validCode)
        assertEquals(0, controller.qrOffers.size)
        assertEquals(PairingScreenState.Home, vm.uiState.value.screen)
        vm.onQrScanned("https://example.com")
        assertNull(vm.uiState.value.scanError)
    }

    @Test
    fun aCodeScannedWhileAnAttemptRunsIsIgnored() = runTest {
        val vm = viewModel()
        controller.state.value = PairingState.Verifying(PairingMethod.QR_CODE, "Desk")
        vm.onQrScanned(validCode)
        assertEquals(0, controller.qrOffers.size)
    }

    @Test
    fun qrWaitCountsDownAndShowsTheAddressAfterFifteenSeconds() = runTest {
        val vm = viewModel()
        discovery.endpoints.value = listOf(
            InetSocketAddress(InetAddress.getByName("192.168.1.5"), 27380),
            InetSocketAddress(InetAddress.getByName("fd00::5"), 27380),
        )
        vm.startScan()
        vm.onQrScanned(validCode)
        controller.state.value = PairingState.AwaitingDesktop.Qr(controller.qrOffers.single(), 1_000_000 + 300_000)

        var screen = vm.uiState.value.screen as PairingScreenState.WaitingForQr
        assertEquals(300, screen.secondsLeft)
        assertEquals(false, screen.showAddressHint)
        assertEquals(emptyList<String>(), screen.endpoints)

        advanceTimeBy(15_500)
        screen = vm.uiState.value.screen as PairingScreenState.WaitingForQr
        assertEquals(true, screen.showAddressHint)
        assertEquals(2, screen.endpoints.size)
        assertEquals("[fd00:0:0:0:0:0:0:5]:27380", screen.endpoints.last())
        assertEquals("192.168.1.5:27380", screen.endpoints.first())
        assertEquals(285, screen.secondsLeft)
    }

    @Test
    fun sasWaitShowsNameAttemptsAndLastFailure() = runTest {
        val vm = viewModel()
        discovery.registeredName.value = "l2l-0a1b2c3d"
        vm.startSasPairing()
        assertEquals(1, controller.sasStarts)
        controller.state.value = PairingState.AwaitingDesktop.Sas(1_000_000 + 120_000, attemptsLeft = 2, lastFailure = PairingFailure.DECLINED)
        val screen = vm.uiState.value.screen as PairingScreenState.WaitingForSas
        assertEquals("l2l-0a1b2c3d", screen.instanceName)
        assertEquals(2, screen.attemptsLeft)
        assertEquals(120, screen.secondsLeft)
        assertEquals(PairingFailure.DECLINED, screen.lastFailure)
    }

    @Test
    fun confirmCodeShowsTheFormattedCodeAndPassesTheAnswer() = runTest {
        val vm = viewModel()
        controller.state.value = PairingState.ConfirmCode(SasCode(42917), "Desk")
        assertEquals(PairingScreenState.ConfirmCode("042 917", "Desk"), vm.uiState.value.screen)
        vm.confirmCode(true)
        vm.confirmCode(false)
        assertEquals(listOf(true, false), controller.answers)
    }

    @Test
    fun replacingARecordWithTheSameIdAsksFirst() = runTest {
        val vm = viewModel()
        controller.state.value = PairingState.ConfirmReplace(desk, "Desk 2")
        assertEquals(PairingScreenState.ConfirmReplace("Desk", "Desk 2"), vm.uiState.value.screen)
        vm.confirmReplace(false)
        assertEquals(listOf(false), controller.replaceAnswers)
    }

    @Test
    fun resultsAndDismissAndUnpair() = runTest {
        val vm = viewModel()
        controller.state.value = PairingState.Failed(PairingFailure.TIMED_OUT)
        assertEquals(PairingScreenState.Failed(PairingFailure.TIMED_OUT), vm.uiState.value.screen)
        controller.state.value = PairingState.Paired(desk)
        assertEquals(PairingScreenState.Paired(desk), vm.uiState.value.screen)
        vm.dismiss()
        assertEquals(PairingScreenState.Home, vm.uiState.value.screen)
        vm.unpair(desk.fingerprint)
        assertEquals(listOf(desk.fingerprint), controller.unpaired)
    }

    @Test
    fun connectedComputersAreListedWhileTheyHaveASession() = runTest {
        repository.save(desk)
        val vm = viewModel()
        assertEquals(emptyList<PairedDevice>(), vm.uiState.value.connected)

        connections.connected.value = setOf(desk.fingerprint)
        assertEquals(listOf(desk), vm.uiState.value.connected)

        connections.connected.value = emptySet()
        assertEquals(emptyList<PairedDevice>(), vm.uiState.value.connected)
    }

    @Test
    fun theBackgroundConnectionCanBeTurnedOffAndOn() = runTest {
        val vm = viewModel()
        assertEquals(true, vm.uiState.value.connectionEnabled)
        vm.setConnectionEnabled(false)
        assertEquals(false, vm.uiState.value.connectionEnabled)
    }

    @Test
    fun cancelLeavesScanningAndStopsTheAttempt() = runTest {
        val vm = viewModel()
        vm.startScan()
        vm.cancel()
        assertEquals(PairingScreenState.Home, vm.uiState.value.screen)
        assertEquals(1, controller.cancels)
    }

    private class FakeController : PairingController {
        override val state = MutableStateFlow<PairingState>(PairingState.Idle)
        val qrOffers = mutableListOf<QrPairingOffer>()
        var sasStarts = 0
        var cancels = 0
        val answers = mutableListOf<Boolean>()
        val replaceAnswers = mutableListOf<Boolean>()
        val unpaired = mutableListOf<KeyFingerprint>()

        override fun startQrPairing(offer: QrPairingOffer) {
            qrOffers += offer
        }
        override fun startSasPairing() {
            sasStarts++
        }
        override fun confirmCode(matches: Boolean) {
            answers += matches
        }
        override fun confirmReplace(replace: Boolean) {
            replaceAnswers += replace
        }
        override fun cancel() {
            cancels++
            state.value = PairingState.Idle
        }
        override fun dismiss() {
            state.value = PairingState.Idle
        }
        override suspend fun unpair(fingerprint: KeyFingerprint) {
            unpaired += fingerprint
        }
    }

    private class FakeConnections : DesktopConnections {
        override val listeningPort = MutableStateFlow<Int?>(null)
        override val connected = MutableStateFlow<Set<KeyFingerprint>>(emptySet())
    }

    private class FakeConnection : ConnectionSwitch {
        override val enabled = MutableStateFlow(true)
        override fun setEnabled(enabled: Boolean) {
            this.enabled.value = enabled
        }
    }

    private class FakeDiscovery : DiscoveryStatus {
        override val registeredName = MutableStateFlow<String?>(null)
        override val endpoints = MutableStateFlow<List<InetSocketAddress>>(emptyList())
    }

    private class FakePairedDeviceRepository : PairedDeviceRepository {
        override val pairedDevices = MutableStateFlow<List<PairedDevice>>(emptyList())
        override suspend fun save(device: PairedDevice) {
            pairedDevices.value += device
        }
        override suspend fun forget(fingerprint: KeyFingerprint) {
            pairedDevices.value = pairedDevices.value.filterNot { it.fingerprint == fingerprint }
        }
    }
}
