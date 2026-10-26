package dev.link2linux.android.core.data.store

import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.security.SecretBox
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.security.GeneralSecurityException
import java.util.UUID

/**
 * Pairings and this installation's identity, in one file sealed with a
 * [SecretBox] whose key lives in the Android Keystore. The file sits in the
 * app's no-backup directory, so it never leaves the phone; neither does the
 * Keystore key, so a copy of the file is useless elsewhere.
 *
 * If the file cannot be opened, for example because the Keystore lost its
 * key, the store starts empty with a new device id: the identity key is
 * gone in that case too, and the phone has to pair again anyway.
 *
 * Writes go to a temporary file that then replaces the old one, so a crash
 * leaves either the old or the new content.
 */
class PersistentPairingStore(
    private val file: File,
    private val box: SecretBox,
    private val defaultName: () -> String,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : PairedDeviceRepository, LocalDeviceRepository {

    private val mutex = Mutex()
    private val content = MutableStateFlow<PairingStoreContent?>(null)

    override val pairedDevices: Flow<List<PairedDevice>> = flow {
        loaded()
        emitAll(content.filterNotNull().map { it.devices })
    }.distinctUntilChanged()

    override suspend fun save(device: PairedDevice) = modify { current ->
        current.copy(devices = current.devices.filterNot { it.accepts(device.fingerprint) } + device)
    }

    override suspend fun forget(fingerprint: KeyFingerprint) = modify { current ->
        current.copy(devices = current.devices.filterNot { it.fingerprint == fingerprint })
    }

    override suspend fun find(key: KeyFingerprint): PairedDevice? = loaded().devices.firstOrNull { it.accepts(key) }

    override suspend fun update(fingerprint: KeyFingerprint, change: (PairedDevice) -> PairedDevice) = modify { current ->
        val old = current.devices.firstOrNull { it.fingerprint == fingerprint } ?: return@modify current
        val new = change(old)
        current.copy(devices = current.devices.map { if (it === old) new else it }.filter { it === new || !it.accepts(new.fingerprint) })
    }

    override suspend fun localDevice(): Device {
        val current = loaded()
        return Device(current.localId, name = current.localName ?: defaultName(), kind = DeviceKind.PHONE)
    }

    override suspend fun setName(name: String?) {
        if (name != null) {
            require(name.isNotBlank()) { "A device name must not be blank" }
            require(name.toByteArray().size <= LocalDeviceRepository.MAX_NAME_BYTES) { "A device name has at most 64 bytes" }
        }
        modify { it.copy(localName = name) }
    }

    private suspend fun loaded(): PairingStoreContent = content.value ?: mutex.withLock { loadLocked() }

    private suspend fun modify(change: (PairingStoreContent) -> PairingStoreContent) = mutex.withLock {
        val current = loadLocked()
        val next = change(current)
        if (next != current) {
            write(next)
            content.value = next
        }
    }

    private suspend fun loadLocked(): PairingStoreContent = content.value ?: withContext(ioDispatcher) {
        read() ?: PairingStoreContent(DeviceId(UUID.randomUUID().toString()), localName = null, devices = emptyList())
            .also { writeBlocking(it) }
    }.also { content.value = it }

    private fun read(): PairingStoreContent? {
        if (!file.exists()) return null
        return try {
            PairingStoreCodec.decode(box.open(file.readBytes(), ASSOCIATED_DATA))
        } catch (e: IOException) {
            null
        } catch (e: GeneralSecurityException) {
            null
        } catch (e: IllegalArgumentException) {
            null
        }
    }

    private suspend fun write(content: PairingStoreContent) = withContext(ioDispatcher) { writeBlocking(content) }

    private fun writeBlocking(content: PairingStoreContent) {
        file.parentFile?.mkdirs()
        val sealed = box.seal(PairingStoreCodec.encode(content), ASSOCIATED_DATA)
        val temporary = File(file.parentFile, "${file.name}.new")
        FileOutputStream(temporary).use { out ->
            out.write(sealed)
            out.fd.sync()
        }
        if (!temporary.renameTo(file)) throw IOException("Could not replace $file")
    }

    private companion object {
        /** Binds the sealed bytes to this file's purpose and format. */
        val ASSOCIATED_DATA = "link2linux pairing store v1".toByteArray()
    }
}
