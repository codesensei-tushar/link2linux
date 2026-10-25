package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class InMemoryRepositoriesTest {

    private fun paired(id: String, fingerprint: String = "aa") = PairedDevice(
        device = Device(DeviceId(id), name = id, kind = DeviceKind.DESKTOP),
        certificateSha256 = fingerprint,
        pairedAtEpochMillis = 0,
    )

    @Test
    fun startsWithNoPairedDevices() = runBlocking {
        assertTrue(InMemoryPairedDeviceRepository().pairedDevices.first().isEmpty())
    }

    @Test
    fun pairingAgainReplacesTheOldEntry() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk", fingerprint = "aa"))
        repository.save(paired("laptop"))
        repository.save(paired("desk", fingerprint = "bb"))

        val devices = repository.pairedDevices.first()
        assertEquals(listOf("laptop", "desk"), devices.map { it.device.id.value })
        assertEquals("bb", devices.last().certificateSha256)
    }

    @Test
    fun forgetRemovesTheDevice() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk"))
        repository.forget(DeviceId("desk"))
        assertTrue(repository.pairedDevices.first().isEmpty())
    }

    @Test
    fun featuresStartOffAndToggle() = runBlocking {
        val settings = InMemoryFeatureSettingsRepository()
        assertTrue(settings.enabledFeatures.first().isEmpty())

        settings.setEnabled(Feature.CLIPBOARD, true)
        settings.setEnabled(Feature.FILES, true)
        settings.setEnabled(Feature.CLIPBOARD, false)

        assertEquals(setOf(Feature.FILES), settings.enabledFeatures.first())
    }

    @Test(expected = IllegalArgumentException::class)
    fun pairingIsNotAToggle() = runBlocking {
        InMemoryFeatureSettingsRepository().setEnabled(Feature.PAIRING, false)
    }
}
