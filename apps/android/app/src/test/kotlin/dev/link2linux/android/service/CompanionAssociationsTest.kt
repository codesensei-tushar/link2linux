package dev.link2linux.android.service

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import org.junit.Assert.assertEquals
import org.junit.Test

class CompanionAssociationsTest {

    private fun desktop(id: String, key: Int, bluetooth: String?) = PairedDevice(
        device = Device(DeviceId(id), id, DeviceKind.DESKTOP),
        fingerprint = KeyFingerprint.of(ByteArray(32) { key.toByte() }),
        pairedAtEpochMillis = 0,
        presenceKey = PresenceKey.of(ByteArray(32) { key.toByte() }),
        bluetoothAddress = bluetooth,
    )

    private val a = desktop("a", 1, "0C:1A:2B:3C:4D:5E")
    private val b = desktop("b", 2, "0C:1A:2B:3C:4D:5F")
    private val associated = listOf("0c:1a:2b:3c:4d:5e", "0c:1a:2b:3c:4d:5f")

    @Test
    fun unpairingADesktopRemovesItsAssociationOnly() {
        assertEquals(listOf("0c:1a:2b:3c:4d:5e"), CompanionAssociations.unneeded(listOf(a, b), listOf(b), associated))
    }

    @Test
    fun withNothingPairedEveryAssociationGoes() {
        // The last unpair, or a store that had to start afresh while the app was not running.
        assertEquals(associated, CompanionAssociations.unneeded(listOf(a, b), emptyList(), associated))
        assertEquals(associated, CompanionAssociations.unneeded(null, emptyList(), associated))
    }

    @Test
    fun aPairingThatStaysKeepsItsAssociation() {
        // Replaced by a pairing with a new key: the same desktop, so the same association.
        val replaced = desktop("a", 3, "0C:1A:2B:3C:4D:5E")
        assertEquals(emptyList<String>(), CompanionAssociations.unneeded(listOf(a, b), listOf(replaced, b), associated))
        // The desktop reported no Bluetooth address for now; it may report it again.
        val quiet = a.copy(bluetoothAddress = null)
        assertEquals(emptyList<String>(), CompanionAssociations.unneeded(listOf(a, b), listOf(quiet, b), associated))
        // At the start nothing is known to have gone.
        assertEquals(emptyList<String>(), CompanionAssociations.unneeded(null, listOf(b), associated))
    }
}
