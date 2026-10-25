package dev.link2linux.android.feature.pairing

import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.PairedDevice
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class PairingViewModelTest {

    private val repository = FakePairedDeviceRepository()

    @Before
    fun setMain() = Dispatchers.setMain(UnconfinedTestDispatcher())

    @After
    fun resetMain() = Dispatchers.resetMain()

    @Test
    fun showsEmptyStateUntilAComputerIsPaired() = runTest {
        val viewModel = PairingViewModel(repository)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { viewModel.uiState.collect {} }
        assertEquals(PairingUiState.NotPaired, viewModel.uiState.value)

        val desk = PairedDevice(Device(DeviceId("desk"), "Desk", DeviceKind.DESKTOP), "ab12", 0)
        repository.save(desk)

        assertEquals(PairingUiState.Paired(listOf(desk)), viewModel.uiState.value)
    }

    private class FakePairedDeviceRepository : PairedDeviceRepository {
        override val pairedDevices = MutableStateFlow<List<PairedDevice>>(emptyList())
        override suspend fun save(device: PairedDevice) {
            pairedDevices.value += device
        }
        override suspend fun forget(id: DeviceId) {
            pairedDevices.value = pairedDevices.value.filterNot { it.device.id == id }
        }
    }
}
