package dev.link2linux.android.core.network.pairing

import com.google.protobuf.ByteString
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.network.FrameCodec
import dev.link2linux.android.core.network.tls.TestDesktop
import dev.link2linux.android.core.security.PairingCrypto
import dev.link2linux.protocol.v1.DeviceKind
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.PairingOutcome
import dev.link2linux.protocol.v1.PairingResult
import dev.link2linux.protocol.v1.hello
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import javax.net.ssl.SSLSocket

/**
 * The desktop's end of a test connection, driven step by step from the
 * test with blocking calls.
 */
class DesktopPeer(private val socket: SSLSocket, private val desktop: TestDesktop) {

    private val codec = FrameCodec()
    private val input = BufferedInputStream(socket.inputStream)
    private val output = BufferedOutputStream(socket.outputStream)
    private var seq = 0L

    val phoneKey: KeyFingerprint get() = checkNotNull(desktop.seenPhone)

    fun send(fill: Envelope.Builder.() -> Unit) {
        codec.write(Envelope.newBuilder().apply(fill).setSeq(++seq).build(), output)
        output.flush()
    }

    /** The phone's next envelope, or null when it closed the connection. */
    fun read(): Envelope? = runCatching { codec.read(input) }.getOrNull()

    fun sendHello(method: PairingMethod, deviceId: String = DEVICE_ID) = send {
        hello = hello {
            protocolVersion = 1
            this.deviceId = deviceId
            deviceName = "Test desktop"
            kind = DeviceKind.DEVICE_KIND_DESKTOP
            pairingMethod = method
        }
    }

    fun readHello(): Hello = checkNotNull(read()).hello

    fun sendResult(outcome: PairingOutcome) = send { pairingResult = PairingResult.newBuilder().setOutcome(outcome).build() }

    /** This side's exporter value, computed independently of the phone. */
    fun ekm(label: String): ByteArray =
        checkNotNull(desktop.export(socket, label, PairingCrypto.exporterContext(checkNotNull(desktop.fingerprint), phoneKey)))

    fun close() = socket.close()

    companion object {
        const val DEVICE_ID = "6f1c2a9e-3b7d-4e58-9a0c-d2b4f8e1a7c3"

        fun bytes(value: ByteArray): ByteString = ByteString.copyFrom(value)
    }
}
