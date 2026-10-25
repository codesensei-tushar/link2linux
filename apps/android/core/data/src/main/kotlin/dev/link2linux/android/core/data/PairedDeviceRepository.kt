package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.PairedDevice
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import javax.inject.Inject
import javax.inject.Singleton

/** Computers this phone trusts, with the certificate fingerprint saved at pairing. */
interface PairedDeviceRepository {

    /** Paired computers, in the order they were paired. */
    val pairedDevices: Flow<List<PairedDevice>>

    /** Saves a newly paired computer, replacing an earlier pairing with the same id. */
    suspend fun save(device: PairedDevice)

    /** Forgets a computer. Its next connection attempt is refused. */
    suspend fun forget(id: DeviceId)
}

/** Keeps pairings in memory until persistent storage lands. */
@Singleton
internal class InMemoryPairedDeviceRepository @Inject constructor() : PairedDeviceRepository {

    private val devices = MutableStateFlow<List<PairedDevice>>(emptyList())

    override val pairedDevices: StateFlow<List<PairedDevice>> = devices.asStateFlow()

    override suspend fun save(device: PairedDevice) = devices.update { current ->
        current.filterNot { it.device.id == device.device.id } + device
    }

    override suspend fun forget(id: DeviceId) = devices.update { current ->
        current.filterNot { it.device.id == id }
    }
}
