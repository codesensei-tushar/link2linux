package dev.link2linux.android.core.data.store

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress

/** Everything the pairing store keeps: this installation's identity and its pairings. */
internal data class PairingStoreContent(
    val localId: DeviceId,
    /** The name the user chose, or null for the default. */
    val localName: String?,
    val devices: List<PairedDevice>,
)

/**
 * The store's byte format, before sealing: a format version, then fields in
 * a fixed order. Addresses are kept as IP literals, so reading them back
 * never looks anything up.
 */
internal object PairingStoreCodec {

    private const val FORMAT = 1
    private const val IPV6_SIZE = 16

    fun encode(content: PairingStoreContent): ByteArray {
        val bytes = ByteArrayOutputStream()
        DataOutputStream(bytes).run {
            writeByte(FORMAT)
            writeUTF(content.localId.value)
            writeOptional(content.localName) { writeUTF(it) }
            writeShort(content.devices.size)
            content.devices.forEach { writeDevice(it) }
        }
        return bytes.toByteArray()
    }

    /** @throws IOException if [bytes] are not in a format this version knows. */
    fun decode(bytes: ByteArray): PairingStoreContent = DataInputStream(ByteArrayInputStream(bytes)).run {
        val format = readUnsignedByte()
        if (format != FORMAT) throw IOException("Unknown pairing store format $format")
        val content = PairingStoreContent(
            localId = DeviceId(readUTF()),
            localName = readOptional { readUTF() },
            devices = List(readUnsignedShort()) { readDevice() },
        )
        if (read() != -1) throw IOException("Trailing bytes in the pairing store")
        content
    }

    private fun DataOutputStream.writeDevice(paired: PairedDevice) {
        writeUTF(paired.device.id.value)
        writeUTF(paired.device.name)
        writeUTF(paired.device.kind.name)
        write(paired.fingerprint.toByteArray())
        writeLong(paired.pairedAtEpochMillis)
        write(paired.presenceKey.toByteArray())
        writeOptional(paired.nextFingerprint) { write(it.toByteArray()) }
        writeOptional(paired.lastConnectedEpochMillis) { writeLong(it) }
        writeOptional(paired.bluetoothAddress) { writeUTF(it) }
        writeByte(paired.lastKnownAddresses.size)
        paired.lastKnownAddresses.forEach { address ->
            val ip = address.address?.address ?: throw IOException("Unresolved address $address")
            writeByte(ip.size)
            write(ip)
            if (ip.size == IPV6_SIZE) writeInt((address.address as Inet6Address).scopeId)
            writeShort(address.port)
        }
    }

    private fun DataInputStream.readDevice(): PairedDevice = PairedDevice(
        device = Device(DeviceId(readUTF()), name = readUTF(), kind = DeviceKind.valueOf(readUTF())),
        fingerprint = KeyFingerprint.of(readBytes(KeyFingerprint.SIZE)),
        pairedAtEpochMillis = readLong(),
        presenceKey = PresenceKey.of(readBytes(PresenceKey.SIZE)),
        nextFingerprint = readOptional { KeyFingerprint.of(readBytes(KeyFingerprint.SIZE)) },
        lastConnectedEpochMillis = readOptional { readLong() },
        bluetoothAddress = readOptional { readUTF() },
        lastKnownAddresses = List(readUnsignedByte()) {
            val ip = readBytes(readUnsignedByte())
            val host = if (ip.size == IPV6_SIZE) {
                readInt().let { scope -> if (scope == 0) InetAddress.getByAddress(ip) else Inet6Address.getByAddress(null, ip, scope) }
            } else {
                InetAddress.getByAddress(ip)
            }
            InetSocketAddress(host, readUnsignedShort())
        },
    )

    private fun DataInputStream.readBytes(count: Int): ByteArray = ByteArray(count).also { readFully(it) }

    private inline fun <T : Any> DataOutputStream.writeOptional(value: T?, writeValue: DataOutputStream.(T) -> Unit) {
        writeBoolean(value != null)
        if (value != null) writeValue(value)
    }

    private inline fun <T : Any> DataInputStream.readOptional(readValue: DataInputStream.() -> T): T? =
        if (readBoolean()) readValue() else null
}
