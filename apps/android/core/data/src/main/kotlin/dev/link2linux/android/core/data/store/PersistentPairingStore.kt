package dev.link2linux.android.core.data.store

import android.security.keystore.KeyPermanentlyInvalidatedException
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.findByKey
import dev.link2linux.android.core.data.saving
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.security.SecretBox
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.retryWhen
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.security.GeneralSecurityException
import java.util.UUID
import javax.crypto.AEADBadTagException

/**
 * Pairings and this installation's identity, in one file sealed with a
 * [SecretBox] whose key lives in the Android Keystore. The file sits in the
 * app's no-backup directory, so it never leaves the phone; neither does the
 * Keystore key, so a copy of the file is useless elsewhere.
 *
 * If the file can never be opened again, because the Keystore lost or
 * invalidated its key or the file was damaged, the store starts empty with
 * a new device id: the identity key is gone in that case too, and the phone
 * has to pair again anyway. The old file is kept beside it as
 * `<name>.unreadable`. Any other error, which may pass, is thrown and the
 * file is left alone, so the next read can try again.
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
    }.retryWhen { cause, attempt ->
        // Every error that gets here may pass, as the ones that cannot set the file aside instead,
        // including the Keystore's own (ProviderException): read the file again, a little later each time.
        val passing = cause is Exception && cause !is CancellationException
        if (passing) delay((RETRY_MILLIS shl attempt.coerceAtMost(6).toInt()).coerceAtMost(MAX_RETRY_MILLIS))
        passing
    }.distinctUntilChanged()

    override suspend fun save(device: PairedDevice) = modify { current ->
        current.copy(devices = current.devices.saving(device))
    }

    override suspend fun forget(fingerprint: KeyFingerprint) = modify { current ->
        current.copy(devices = current.devices.filterNot { it.fingerprint == fingerprint })
    }

    override suspend fun find(key: KeyFingerprint): PairedDevice? = loaded().devices.findByKey(key)

    override suspend fun update(fingerprint: KeyFingerprint, change: (PairedDevice) -> PairedDevice) = modify { current ->
        val old = current.devices.firstOrNull { it.fingerprint == fingerprint } ?: return@modify current
        val new = change(old)
        val devices = current.devices.mapNotNull {
            when {
                it === old -> new
                // Only a record with the same current key gives way; one that named it as its next key loses that claim.
                it.fingerprint == new.fingerprint -> null
                it.nextFingerprint == new.fingerprint -> it.copy(nextFingerprint = null)
                else -> it
            }
        }
        current.copy(devices = devices)
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

    /** The stored content, or null to start afresh. Throws on errors that may pass. */
    private fun read(): PairingStoreContent? {
        if (!file.exists()) return null
        val sealed = file.readBytes()
        val plain = try {
            box.open(sealed, ASSOCIATED_DATA)
        } catch (e: GeneralSecurityException) {
            if (!isPermanent(e)) throw e
            return setAside()
        }
        // The bytes are this installation's own; a format it cannot read will never become readable.
        return try {
            PairingStoreCodec.decode(plain)
        } catch (e: IOException) {
            setAside()
        } catch (e: IllegalArgumentException) {
            setAside()
        }
    }

    /** The sealed bytes failed their check, or the key that sealed them is gone for good. */
    private fun isPermanent(e: GeneralSecurityException): Boolean =
        e is AEADBadTagException || e is KeyPermanentlyInvalidatedException

    /** Moves the unreadable file out of the way, so starting afresh never destroys it. */
    private fun setAside(): PairingStoreContent? {
        val aside = File(file.parentFile, "${file.name}.unreadable")
        aside.delete()
        if (!file.renameTo(aside)) throw IOException("Could not move the unreadable $file aside")
        return null
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

        const val RETRY_MILLIS = 100L
        const val MAX_RETRY_MILLIS = 5_000L
    }
}
