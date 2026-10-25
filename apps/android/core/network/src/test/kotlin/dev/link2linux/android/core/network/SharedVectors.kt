package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.DeviceKind
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.ping
import dev.link2linux.protocol.v1.pong
import java.io.File

/**
 * The shared wire-format vectors in `proto/testdata/v1`, which the desktop
 * tests check too. [envelopes] mirrors the `.txtpb` sources there; the build
 * passes the directory in the `l2l.testdata.dir` system property.
 */
object SharedVectors {

    val envelopes: List<Envelope> = listOf(
        envelope {
            seq = 1
            hello = hello {
                protocolVersion = 1
                deviceId = "6f1c2a9e-3b7d-4e58-9a0c-d2b4f8e1a7c3"
                deviceName = "Büro workstation"
                kind = DeviceKind.DEVICE_KIND_DESKTOP
                features += listOf("notifications", "messages", "calls", "photos", "files", "clipboard", "screen")
            }
        },
        envelope {
            seq = 2
            ping = ping { nonce = 1_234_567_890_123 }
        },
        envelope {
            seq = 3
            pong = pong { nonce = 1_234_567_890_123 }
        },
    )

    /** `v1/frames.bin`, or null in a checkout that does not have the vectors. */
    fun framesFile(): File? = System.getProperty("l2l.testdata.dir")
        ?.let { File(it, "v1/frames.bin") }
        ?.takeIf(File::isFile)
}

fun hex(text: String): ByteArray = text.replace(" ", "").chunked(2).map { it.toInt(16).toByte() }.toByteArray()
