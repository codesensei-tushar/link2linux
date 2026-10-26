package dev.link2linux.android.core.model

import java.net.InetSocketAddress

/** Stable identifier a device announces in `Hello.device_id`. */
@JvmInline
value class DeviceId(val value: String) {
    init {
        require(value.isNotBlank()) { "DeviceId must not be blank" }
    }

    override fun toString(): String = value
}

/** Mirrors `link2linux.v1.DeviceKind`. */
enum class DeviceKind { DESKTOP, PHONE }

/** A device as it introduces itself in its `Hello`. */
data class Device(
    val id: DeviceId,
    val name: String,
    val kind: DeviceKind,
    val features: Set<Feature> = emptySet(),
)

/**
 * A desktop this phone trusts: its pairing record. Records are found by
 * key, never by the id a desktop claims: the phone accepts a connection only
 * from the key behind [fingerprint] or [nextFingerprint], and then requires
 * the desktop's `Hello.device_id` to equal [device]'s id.
 */
data class PairedDevice(
    val device: Device,
    /** The desktop's current key fingerprint. */
    val fingerprint: KeyFingerprint,
    val pairedAtEpochMillis: Long,
    /** The key behind the phone's presence tags for this desktop. */
    val presenceKey: PresenceKey,
    /** The key the desktop announced with `KeyRotation`, until it first connects with it. */
    val nextFingerprint: KeyFingerprint? = null,
    /** When the last session with the desktop started, or null before the first one. */
    val lastConnectedEpochMillis: Long? = null,
    /** The desktop's Bluetooth LE address as "0C:1A:2B:3C:4D:5E", or null. */
    val bluetoothAddress: String? = null,
    /** Where the phone sends beacons for this desktop: the QR code's addresses, then the last session's. */
    val lastKnownAddresses: List<InetSocketAddress> = emptyList(),
) {
    /** Whether a connection with [key] belongs to this desktop. */
    fun accepts(key: KeyFingerprint): Boolean = key == fingerprint || key == nextFingerprint
}
