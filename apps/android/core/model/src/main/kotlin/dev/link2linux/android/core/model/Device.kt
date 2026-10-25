package dev.link2linux.android.core.model

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
 * A desktop this phone trusts. Pairing stores the peer's certificate
 * fingerprint, and later sessions only accept that certificate.
 */
data class PairedDevice(
    val device: Device,
    /** SHA-256 of the peer's DER-encoded certificate, lowercase hex. */
    val certificateSha256: String,
    val pairedAtEpochMillis: Long,
)
