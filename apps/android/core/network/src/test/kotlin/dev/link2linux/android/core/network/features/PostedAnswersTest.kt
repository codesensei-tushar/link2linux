package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.clipboard.ClipText
import dev.link2linux.android.core.clipboard.ClipboardEngine
import dev.link2linux.android.core.clipboard.ClipboardWriter
import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.StuckTransport
import dev.link2linux.android.core.network.pairing.PairedSession
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.clipboardPush
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.notificationSyncRequest
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList
import dev.link2linux.protocol.v1.DeviceKind as WireDeviceKind

/** The answers a session gives while it reads go out without the reader waiting for its writer. */
class PostedAnswersTest {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val clipboard = CopyOnWriteArrayList<ClipText>()
    private val engine = ClipboardEngine(scope, ClipboardWriter { clipboard += it }, MutableStateFlow(true))
    private val devices = InMemoryPairedDeviceRepository()
    private val transport = StuckTransport()
    private val desktopKey = KeyFingerprint.of(ByteArray(32) { 3 })

    @After
    fun tearDown() {
        transport.close()
        scope.cancel()
    }

    private fun request(seq: Long, fill: Envelope.Builder.() -> Unit): Envelope =
        Envelope.newBuilder().setSeq(seq).setExpectsReply(true).apply(fill).build()

    @Test
    fun answersGivenWhileReadingDoNotWaitForAStuckWriter() = runBlocking {
        devices.save(
            PairedDevice(
                device = Device(DeviceId("desk"), "Desk", DeviceKind.DESKTOP),
                fingerprint = desktopKey,
                pairedAtEpochMillis = 0,
                presenceKey = PresenceKey.of(ByteArray(32) { 9 }),
            ),
        )
        transport.feed(
            envelope {
                seq = 1
                hello = hello {
                    protocolVersion = 1
                    deviceId = "desk"
                    deviceName = "Desk"
                    kind = WireDeviceKind.DEVICE_KIND_DESKTOP
                    features += "clipboard"
                }
            },
        )
        val session = Session(transport)
        withTimeout(5_000) {
            session.handshake(
                hello {
                    protocolVersion = 1
                    deviceId = "phone-1"
                    deviceName = "Phone"
                    kind = WireDeviceKind.DEVICE_KIND_PHONE
                    features += "clipboard"
                },
            )
        }
        session.markPaired()
        scope.launch { PairedSession(devices, ClipboardSessionHooks(engine)).run(desktopKey, KeyFingerprint.of(ByteArray(32) { 1 }), session) }
        withTimeout(5_000) { while (transport.envelopes().none { it.hasPresenceUpdate() }) delay(10) }

        // The desktop stops reading: the phone's writer sticks on its next frame.
        transport.stick()
        session.post { ping = ping { nonce = 1 } }
        transport.awaitBlockedWrite()

        // A push, a request no feature takes, and another push.
        transport.feed(
            request(2) { clipboardPush = clipboardPush { text = "first" } },
            request(3) { notificationSyncRequest = notificationSyncRequest { } },
            request(4) { clipboardPush = clipboardPush { text = "second" } },
        )

        // The reader went on past the first two answers.
        withTimeout(5_000) { while (clipboard.size < 2) delay(10) }
        assertEquals(listOf("first", "second"), clipboard.map { it.text })

        transport.release()
        val answers = withTimeout(5_000) {
            var replies: List<Envelope>
            do {
                delay(10)
                replies = transport.envelopes().filter { it.replyTo != 0L }
            } while (replies.size < 3)
            replies
        }
        assertEquals(listOf(2L, 3L, 4L), answers.map { it.replyTo })
        assertEquals(PayloadCase.ACK, answers[0].payloadCase)
        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, answers[1].errorReply.code)
        assertEquals(PayloadCase.ACK, answers[2].payloadCase)
    }
}
