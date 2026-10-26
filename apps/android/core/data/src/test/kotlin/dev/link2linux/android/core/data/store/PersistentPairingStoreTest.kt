package dev.link2linux.android.core.data.store

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.security.AesGcmSecretBox
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress
import javax.crypto.KeyGenerator

/** The store with a software key; on a phone the key lives in the Keystore. */
class PersistentPairingStoreTest {

    @get:Rule
    val folder = TemporaryFolder()

    private val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    private val box = AesGcmSecretBox { key }
    private val file by lazy { File(folder.root, "pairing/pairings.bin") }

    private fun store(box: AesGcmSecretBox = this.box) = PersistentPairingStore(file, box, defaultName = { "Pixel" })

    private fun fingerprint(byte: Int) = KeyFingerprint.of(ByteArray(32) { byte.toByte() })

    private val desk = PairedDevice(
        device = Device(DeviceId("6f1c2a9e-3b7d-4e58-9a0c-d2b4f8e1a7c3"), "Büro workstation", DeviceKind.DESKTOP),
        fingerprint = fingerprint(1),
        pairedAtEpochMillis = 1_793_318_400_000,
        presenceKey = PresenceKey.of(ByteArray(32) { (0xc0 + it).toByte() }),
        nextFingerprint = fingerprint(2),
        lastConnectedEpochMillis = 1_793_318_500_000,
        bluetoothAddress = "0C:1A:2B:3C:4D:5E",
        lastKnownAddresses = listOf(
            InetSocketAddress(InetAddress.getByName("192.168.1.20"), 27381),
            InetSocketAddress(InetAddress.getByName("fd00::20"), 27381),
            InetSocketAddress(Inet6Address.getByAddress(null, InetAddress.getByName("fe80::1").address, 3), 27381),
        ),
    )
    private val laptop = PairedDevice(
        device = Device(DeviceId("laptop"), "Laptop", DeviceKind.DESKTOP),
        fingerprint = fingerprint(3),
        pairedAtEpochMillis = 5,
        presenceKey = PresenceKey.of(ByteArray(32)),
    )

    @Test
    fun pairingsAndIdentitySurviveARestart() = runBlocking {
        val first = store()
        first.save(desk)
        first.save(laptop)
        first.setName("My phone")
        val id = first.localDevice().id

        val second = store()
        assertEquals(listOf(desk, laptop), second.pairedDevices.first())
        assertEquals(Device(id, "My phone", DeviceKind.PHONE), second.localDevice())
        assertEquals(scopeOf(desk), scopeOf(second.pairedDevices.first().first()))
    }

    private fun scopeOf(device: PairedDevice) = (device.lastKnownAddresses.last().address as Inet6Address).scopeId

    @Test
    fun aNewInstallationGetsARandomIdAndTheDefaultName() = runBlocking {
        val device = store().localDevice()
        assertEquals("Pixel", device.name)
        assertEquals(36, device.id.value.length)
        assertEquals(device, store().localDevice())

        val store = store()
        store.setName("Named")
        store.setName(null)
        assertEquals("Pixel", store.localDevice().name)
    }

    @Test
    fun theFileHoldsNoPlaintext() = runBlocking {
        store().save(desk)
        val bytes = file.readBytes().toString(Charsets.ISO_8859_1)
        assertFalse("Büro" in bytes || "workstation" in bytes || "6f1c2a9e" in bytes)
        assertFalse(File(file.parentFile, "pairings.bin.new").exists())
    }

    @Test
    fun aFileSealedWithAnotherKeyStartsOver() = runBlocking {
        store().save(desk)
        val oldId = store().localDevice().id

        val otherKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        val reset = store(AesGcmSecretBox { otherKey })
        assertTrue(reset.pairedDevices.first().isEmpty())
        assertNotEquals(oldId, reset.localDevice().id)
    }

    @Test
    fun aTamperedFileStartsOver() = runBlocking {
        store().save(desk)
        val bytes = file.readBytes()
        bytes[bytes.size / 2] = (bytes[bytes.size / 2].toInt() xor 1).toByte()
        file.writeBytes(bytes)
        assertTrue(store().pairedDevices.first().isEmpty())
    }

    @Test
    fun recordsAreFoundByKeyAndUpdatedInPlace() = runBlocking {
        val store = store()
        store.save(desk)
        store.save(laptop)

        assertEquals(desk, store.find(fingerprint(2)))
        assertNull(store.find(fingerprint(9)))

        store.update(fingerprint(1)) { it.copy(fingerprint = fingerprint(2), nextFingerprint = null) }
        assertEquals(listOf(fingerprint(2), fingerprint(3)), store().pairedDevices.first().map { it.fingerprint })

        store.forget(fingerprint(3))
        assertEquals(listOf(fingerprint(2)), store().pairedDevices.first().map { it.fingerprint })
    }

    @Test
    fun rePairingWithTheSameKeyReplacesTheRecord() = runBlocking {
        val store = store()
        store.save(desk)
        store.save(laptop)
        val again = desk.copy(device = desk.device.copy(name = "Renamed"), nextFingerprint = null)
        store.save(again)
        assertEquals(listOf(laptop, again), store.pairedDevices.first())
    }

    @Test
    fun codecRefusesUnknownFormats() {
        val encoded = PairingStoreCodec.encode(PairingStoreContent(DeviceId("x"), null, listOf(desk)))
        assertEquals(desk, PairingStoreCodec.decode(encoded).devices.single())
        for (broken in listOf(byteArrayOf(2) + encoded.copyOfRange(1, encoded.size), encoded + 0, encoded.copyOf(20))) {
            assertTrue(runCatching { PairingStoreCodec.decode(broken) }.exceptionOrNull() is IOException)
        }
    }

    @Test(expected = IllegalArgumentException::class)
    fun namesLongerThan64BytesAreRefused() = runBlocking {
        store().setName("ü".repeat(33))
    }
}
