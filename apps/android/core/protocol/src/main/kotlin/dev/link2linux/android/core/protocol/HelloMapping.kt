package dev.link2linux.android.core.protocol

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.Feature
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.DeviceKind as WireDeviceKind

/** Protocol major version this app speaks: the `v1` in `link2linux.v1`. */
const val PROTOCOL_VERSION: Int = 1

/** The Hello this device sends first on every session. */
fun Device.toHello(): Hello = hello {
    protocolVersion = PROTOCOL_VERSION
    deviceId = id.value
    deviceName = name
    kind = when (this@toHello.kind) {
        DeviceKind.DESKTOP -> WireDeviceKind.DEVICE_KIND_DESKTOP
        DeviceKind.PHONE -> WireDeviceKind.DEVICE_KIND_PHONE
    }
    features += this@toHello.features.sorted().map(Feature::wireName)
}

/**
 * The peer as it introduced itself. Feature names this version does not know
 * (sent by a newer peer) are skipped.
 *
 * @throws IllegalArgumentException if the Hello has no device id or kind.
 */
fun Hello.toDevice(): Device = Device(
    id = DeviceId(deviceId),
    name = deviceName,
    kind = when (kind) {
        WireDeviceKind.DEVICE_KIND_DESKTOP -> DeviceKind.DESKTOP
        WireDeviceKind.DEVICE_KIND_PHONE -> DeviceKind.PHONE
        else -> throw IllegalArgumentException("Hello from $deviceId has no device kind")
    },
    features = featuresList.mapNotNullTo(mutableSetOf(), Feature::fromWireName),
)
