package dev.link2linux.android.core.data

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Desktops this phone trusts. A record is identified by the desktop's key
 * fingerprint, never by the device id the desktop claims.
 */
interface PairedDeviceRepository {

    /** Paired desktops, in the order they were paired. */
    val pairedDevices: Flow<List<PairedDevice>>

    /**
     * Saves a pairing, replacing the record with the same current key
     * (re-pairing). A record for another key is kept even when it has the
     * same device id; if it named this key as its next one, it loses that
     * claim, as the key now belongs to [device].
     */
    suspend fun save(device: PairedDevice)

    /** Forgets the desktop with this current key. Its next connection is refused. */
    suspend fun forget(fingerprint: KeyFingerprint)

    /** The record whose current key is [key], else the one whose next key it is, or null. */
    suspend fun find(key: KeyFingerprint): PairedDevice? = pairedDevices.first().findByKey(key)

    /** Replaces the record whose current key is [fingerprint] with [change] of it, if there is one. */
    suspend fun update(fingerprint: KeyFingerprint, change: (PairedDevice) -> PairedDevice) {
        val current = pairedDevices.first().firstOrNull { it.fingerprint == fingerprint } ?: return
        val changed = change(current)
        if (changed.fingerprint != fingerprint) forget(fingerprint)
        save(changed)
    }
}

/** The record whose current key is [key], else the one whose next key it is. A current key always wins. */
fun List<PairedDevice>.findByKey(key: KeyFingerprint): PairedDevice? =
    firstOrNull { it.fingerprint == key } ?: firstOrNull { it.nextFingerprint == key }

/**
 * These records with [device] saved: the record with the same current key
 * is replaced and [device] goes last. A record that only names the key as
 * its next one is kept, without that claim, since the key is [device]'s now.
 */
fun List<PairedDevice>.saving(device: PairedDevice): List<PairedDevice> =
    filter { it.fingerprint != device.fingerprint }.withoutNextKey(device.fingerprint) + device

/** These records with [key] taken out of their next keys. */
private fun List<PairedDevice>.withoutNextKey(key: KeyFingerprint): List<PairedDevice> =
    map { if (it.nextFingerprint == key) it.copy(nextFingerprint = null) else it }

/** Keeps pairings in memory. For tests and previews; the app stores them on disk. */
@Singleton
class InMemoryPairedDeviceRepository @Inject constructor() : PairedDeviceRepository {

    private val devices = MutableStateFlow<List<PairedDevice>>(emptyList())

    override val pairedDevices: StateFlow<List<PairedDevice>> = devices.asStateFlow()

    override suspend fun save(device: PairedDevice) = devices.update { current -> current.saving(device) }

    override suspend fun forget(fingerprint: KeyFingerprint) = devices.update { current ->
        current.filterNot { it.fingerprint == fingerprint }
    }
}
