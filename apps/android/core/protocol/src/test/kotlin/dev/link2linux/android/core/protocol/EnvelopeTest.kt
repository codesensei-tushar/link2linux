package dev.link2linux.android.core.protocol

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.Feature
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.ping
import org.junit.Assert.assertEquals
import org.junit.Test
import dev.link2linux.protocol.v1.DeviceKind as WireDeviceKind

class EnvelopeTest {

    private val phone = Device(
        id = DeviceId("phone-1"),
        name = "Pixel",
        kind = DeviceKind.PHONE,
        features = setOf(Feature.CLIPBOARD, Feature.NOTIFICATIONS),
    )

    @Test
    fun helloEnvelopeRoundTrips() {
        val original = envelope {
            seq = 1
            hello = phone.toHello()
        }

        val parsed = Envelope.parseFrom(original.toByteArray())

        assertEquals(original, parsed)
        assertEquals(Envelope.PayloadCase.HELLO, parsed.payloadCase)
        assertEquals(PROTOCOL_VERSION, parsed.hello.protocolVersion)
        assertEquals(WireDeviceKind.DEVICE_KIND_PHONE, parsed.hello.kind)
        assertEquals(listOf("notifications", "clipboard"), parsed.hello.featuresList)
        assertEquals(phone, parsed.hello.toDevice())
    }

    @Test
    fun pingEnvelopeRoundTrips() {
        val original = envelope {
            seq = 7
            ping = ping { nonce = Long.MAX_VALUE }
        }

        val parsed = Envelope.parseFrom(original.toByteArray())

        assertEquals(original, parsed)
        assertEquals(Long.MAX_VALUE, parsed.ping.nonce)
    }

    @Test
    fun unknownFieldsFromNewerPeersAreTolerated() {
        // seq = 3, then field 99 (varint 1) that this schema does not define.
        val bytes = byteArrayOf(0x08, 0x03, 0x98.toByte(), 0x06, 0x01)

        val parsed = Envelope.parseFrom(bytes)

        assertEquals(3L, parsed.seq)
        assertEquals(Envelope.PayloadCase.PAYLOAD_NOT_SET, parsed.payloadCase)
    }

    @Test
    fun unknownFeatureNamesAreSkipped() {
        val device = hello {
            deviceId = "desk-1"
            deviceName = "Desk"
            kind = WireDeviceKind.DEVICE_KIND_DESKTOP
            features += listOf("files", "hologram")
        }.toDevice()

        assertEquals(setOf(Feature.FILES), device.features)
    }

    @Test(expected = IllegalArgumentException::class)
    fun helloWithoutKindIsRejected() {
        hello { deviceId = "desk-1" }.toDevice()
    }
}
