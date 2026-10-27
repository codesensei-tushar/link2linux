package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class InMemoryRepositoriesTest {

    private fun key(byte: Int) = KeyFingerprint.of(ByteArray(32) { byte.toByte() })

    private fun paired(id: String, fingerprint: KeyFingerprint, next: KeyFingerprint? = null) = PairedDevice(
        device = Device(DeviceId(id), name = id, kind = DeviceKind.DESKTOP),
        fingerprint = fingerprint,
        pairedAtEpochMillis = 0,
        presenceKey = PresenceKey.of(ByteArray(32)),
        nextFingerprint = next,
    )

    @Test
    fun startsWithNoPairedDevices() = runBlocking {
        assertTrue(InMemoryPairedDeviceRepository().pairedDevices.first().isEmpty())
    }

    @Test
    fun pairingAgainWithTheSameKeyReplacesTheOldEntry() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk", key(1)))
        repository.save(paired("laptop", key(2)))
        repository.save(paired("desk-renamed", key(1)))

        val devices = repository.pairedDevices.first()
        assertEquals(listOf("laptop", "desk-renamed"), devices.map { it.device.id.value })
    }

    @Test
    fun aClaimedDeviceIdDoesNotReplaceAnotherKeysRecord() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk", key(1)))
        repository.save(paired("desk", key(2)))

        assertEquals(listOf(key(1), key(2)), repository.pairedDevices.first().map { it.fingerprint })
    }

    @Test
    fun findsByCurrentOrNextKey() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk", key(1), next = key(3)))

        assertEquals("desk", repository.find(key(1))?.device?.id?.value)
        assertEquals("desk", repository.find(key(3))?.device?.id?.value)
        assertNull(repository.find(key(2)))
    }

    @Test
    fun aCurrentKeyWinsOverAnEarlierRecordsNextKey() {
        val first = paired("first", key(1), next = key(2))
        val second = paired("second", key(2))
        assertEquals(second, listOf(first, second).findByKey(key(2)))
        assertEquals(first, listOf(first, second).findByKey(key(1)))
    }

    @Test
    fun savingAKeyTakesItFromAnotherRecordsNextKeyWithoutDroppingThatRecord() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("first", key(1), next = key(2)))
        repository.save(paired("second", key(2)))

        assertEquals(listOf(paired("first", key(1)), paired("second", key(2))), repository.pairedDevices.first())
    }

    @Test
    fun updateCanPromoteTheNextKey() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk", key(1), next = key(3)))
        repository.update(key(1)) { it.copy(fingerprint = key(3), nextFingerprint = null) }

        assertEquals(listOf(paired("desk", key(3))), repository.pairedDevices.first())
    }

    @Test
    fun forgetRemovesTheDevice() = runBlocking {
        val repository = InMemoryPairedDeviceRepository()
        repository.save(paired("desk", key(1)))
        repository.forget(key(1))
        assertTrue(repository.pairedDevices.first().isEmpty())
    }

    @Test
    fun featuresStartAsTheirDefaultAndToggle() = runBlocking {
        val settings = InMemoryFeatureSettingsRepository()
        assertEquals(setOf(Feature.CALLS, Feature.STATUS), settings.enabledFeatures.first())

        settings.setEnabled(Feature.CLIPBOARD, true)
        settings.setEnabled(Feature.FILES, true)
        settings.setEnabled(Feature.CLIPBOARD, false)
        settings.setEnabled(Feature.CALLS, false)
        settings.setEnabled(Feature.STATUS, false)

        assertEquals(setOf(Feature.FILES), settings.enabledFeatures.first())
    }

    @Test(expected = IllegalArgumentException::class)
    fun pairingIsNotAToggle() = runBlocking {
        InMemoryFeatureSettingsRepository().setEnabled(Feature.PAIRING, false)
    }
}
