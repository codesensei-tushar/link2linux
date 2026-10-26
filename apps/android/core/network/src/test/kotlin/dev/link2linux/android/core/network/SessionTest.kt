package dev.link2linux.android.core.network

import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.DeviceKind
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.clipboardPush
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.hello
import dev.link2linux.protocol.v1.notificationPosted
import dev.link2linux.protocol.v1.phoneNotification
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors

/** Sessions over loopback TCP; TLS is the transport's job and is not involved here. */
class SessionTest {

    private val phoneHello = hello {
        protocolVersion = 1
        deviceId = "phone-1"
        deviceName = "Phone"
        kind = DeviceKind.DEVICE_KIND_PHONE
        features += "clipboard"
    }
    private val desktopHello = hello {
        protocolVersion = 1
        deviceId = "desk-1"
        deviceName = "Desk"
        kind = DeviceKind.DEVICE_KIND_DESKTOP
    }

    private val sockets = mutableListOf<Socket>()

    private class SocketTransport(private val socket: Socket) : Transport {
        override val input = socket.getInputStream()
        override val output = socket.getOutputStream()
        override fun close() = socket.close()
    }

    /**
     * Returns the phone end and the desktop end of a connected pair. With
     * [bufferBytes], both ends' socket buffers are about that small, so one
     * large frame fills them while the other end does not read.
     */
    private fun connectedPair(bufferBytes: Int? = null): Pair<Transport, Transport> {
        ServerSocket().use { server ->
            bufferBytes?.let { server.receiveBufferSize = it }
            server.bind(InetSocketAddress(InetAddress.getLoopbackAddress(), 0), 1)
            val client = Socket()
            bufferBytes?.let {
                client.sendBufferSize = it
                client.receiveBufferSize = it
            }
            client.connect(server.localSocketAddress)
            val accepted = server.accept()
            bufferBytes?.let { accepted.sendBufferSize = it }
            sockets += client
            sockets += accepted
            return SocketTransport(client) to SocketTransport(accepted)
        }
    }

    private val stuckTransports = mutableListOf<StuckTransport>()

    @After
    fun closeSockets() = sockets.forEach(Socket::close)

    @After
    fun closeStuckTransports() = stuckTransports.forEach(StuckTransport::close)

    /** A paired phone session over a [StuckTransport], Hellos exchanged. */
    private suspend fun stuckSession(writeTimeoutMillis: Long = Session.WRITE_TIMEOUT_MILLIS): Pair<Session, StuckTransport> {
        val transport = StuckTransport().also { stuckTransports += it }
        transport.feed(envelope { seq = 1; hello = desktopHello })
        val phone = Session(transport, writeTimeoutMillis = writeTimeoutMillis)
        withTimeout(5_000) { phone.handshake(phoneHello) }
        phone.markPaired()
        return phone to transport
    }

    /** Makes [phone]'s writer stick on a frame the peer does not read. */
    private fun stickWriter(phone: Session, transport: StuckTransport) {
        transport.stick()
        phone.post { ping = ping { nonce = 99 } }
        transport.awaitBlockedWrite()
    }

    @Test
    fun bothSidesExchangeHello() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val phone = Session(phoneEnd)
        val desktop = Session(desktopEnd)

        withTimeout(5_000) {
            val desktopSaw = async { desktop.handshake(desktopHello) }
            assertEquals(desktopHello, phone.handshake(phoneHello))
            assertEquals(phoneHello, desktopSaw.await())
        }
        assertEquals(SessionState.Established(desktopHello, version = 1), phone.state.value)
    }

    @Test
    fun theHighestVersionInBothRangesIsUsed() {
        fun hello(min: Int, max: Int) = hello { minProtocolVersion = min; protocolVersion = max }
        assertEquals(1, Session.negotiateVersion(hello(0, 1), hello(1, 3)))
        assertEquals(2, Session.negotiateVersion(hello(1, 2), hello(2, 3)))
        assertEquals(3, Session.negotiateVersion(hello(0, 3), hello(0, 3)))
        assertNull(Session.negotiateVersion(hello(0, 1), hello(2, 3)))
        assertNull(Session.negotiateVersion(hello(0, 1), hello(0, 2)))
        assertNull(Session.negotiateVersion(hello(0, 0), hello(0, 0)))
    }

    @Test
    fun aPeerThatAlsoSpeaksANewerVersionIsAccepted() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val newer = desktopHello.toBuilder().setProtocolVersion(2).setMinProtocolVersion(1).build()
        FrameCodec().write(envelope { seq = 1; hello = newer }, desktopEnd.output)
        desktopEnd.output.flush()
        val phone = Session(phoneEnd)

        withTimeout(5_000) { phone.handshake(phoneHello) }
        assertEquals(SessionState.Established(newer, version = 1), phone.state.value)
    }

    /** Closes [transport] after [millis] on another thread, so a test of a deadline fails instead of hanging without it. */
    private fun closeLater(transport: Transport, millis: Long) =
        Thread { Thread.sleep(millis); transport.close() }.apply { isDaemon = true; start() }

    @Test
    fun aPeerThatSendsNoHelloIsCutOffAtTheDeadline() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        closeLater(desktopEnd, 5_000)
        val phone = Session(phoneEnd)

        val started = System.nanoTime()
        val error = runCatching { phone.handshake(phoneHello, timeoutMillis = 500) }.exceptionOrNull()
        val tookMillis = (System.nanoTime() - started) / 1_000_000

        assertTrue("got $error", error is SessionException)
        assertTrue("took $tookMillis ms", tookMillis < 3_000)
        assertEquals(SessionState.Closed, phone.state.value)
    }

    @Test
    fun aPeerThatStopsReadingIsClosedAtTheWriteDeadline() = runBlocking {
        val (phoneEnd, _) = connectedPair()
        closeLater(phoneEnd, 10_000)
        val phone = Session(phoneEnd, writeTimeoutMillis = 500).apply { markPaired() }
        val big = desktopHello.toBuilder().setDeviceName("x".repeat(400_000)).build()

        val started = System.nanoTime()
        // The desktop end never reads: once the socket buffers are full a write blocks.
        val error = runCatching { repeat(256) { phone.send { hello = big } } }.exceptionOrNull()
        val tookMillis = (System.nanoTime() - started) / 1_000_000

        assertTrue("got $error", error is SessionException)
        assertTrue("took $tookMillis ms", tookMillis < 8_000)
        assertEquals(SessionState.Closed, phone.state.value)
    }

    @Test
    fun noCommonVersionSendsVersionMismatch() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val newer = desktopHello.toBuilder().setProtocolVersion(3).setMinProtocolVersion(2).build()
        FrameCodec().write(envelope { seq = 1; hello = newer }, desktopEnd.output)
        desktopEnd.output.flush()

        val error = withTimeout(5_000) { runCatching { Session(phoneEnd).handshake(phoneHello) }.exceptionOrNull() }

        assertTrue("got $error", error is VersionMismatchException)
        val desktop = FrameCodec()
        assertTrue(desktop.read(desktopEnd.input)!!.hasHello())
        assertEquals(GoodbyeReason.GOODBYE_REASON_VERSION_MISMATCH, desktop.read(desktopEnd.input)!!.goodbye.reason)
    }

    @Test
    fun aSkippedSequenceNumberIsAProtocolError() = assertProtocolError(
        envelope { seq = 1; hello = desktopHello },
        envelope { seq = 3; ping = ping { nonce = 1 } },
    )

    @Test
    fun aRepeatedSequenceNumberIsAProtocolError() = assertProtocolError(
        envelope { seq = 1; hello = desktopHello },
        envelope { seq = 1; ping = ping { nonce = 1 } },
    )

    @Test
    fun aSecondHelloIsAProtocolError() = assertProtocolError(
        envelope { seq = 1; hello = desktopHello },
        envelope { seq = 2; hello = desktopHello },
    )

    /** The phone reads [fromDesktop] after its Hello, fails, and says why in a Goodbye. */
    private fun assertProtocolError(vararg fromDesktop: Envelope) = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val desktop = FrameCodec()
        fromDesktop.forEach { desktop.write(it, desktopEnd.output) }
        desktopEnd.output.flush()
        val phone = Session(phoneEnd)

        val error = withTimeout(5_000) {
            phone.handshake(phoneHello)
            runCatching { phone.next() }.exceptionOrNull()
        }

        assertTrue("expected SessionException, got $error", error is SessionException)
        assertEquals(SessionState.Closed, phone.state.value)
        val sent = generateSequence { desktop.read(desktopEnd.input) }.toList()
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, sent.last().goodbye.reason)
        assertEquals((1L..sent.size).toList(), sent.map { it.seq })
    }

    @Test
    fun pingIsAnsweredAndFeatureMessagesComeThrough() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val phone = Session(phoneEnd)
        val desktop = FrameCodec()

        withTimeout(5_000) {
            desktop.write(envelope { seq = 1; hello = desktopHello }, desktopEnd.output)
            desktop.write(envelope { seq = 2; ping = ping { nonce = 99 } }, desktopEnd.output)
            // A message with a payload this version does not know yet.
            desktop.write(envelope { seq = 3 }, desktopEnd.output)
            desktopEnd.output.flush()

            phone.handshake(phoneHello)
            val message = phone.incoming().first()

            assertEquals(3L, message.seq)
            assertEquals(1L, desktop.read(desktopEnd.input)?.seq) // the phone's Hello
            val pong = desktop.read(desktopEnd.input)!!
            assertEquals(Envelope.PayloadCase.PONG, pong.payloadCase)
            assertEquals(2L, pong.seq)
            assertEquals(99L, pong.pong.nonce)
        }
    }

    @Test
    fun otherMajorVersionIsRefused() = assertHandshakeFails(
        envelope { seq = 1; hello = desktopHello.toBuilder().setProtocolVersion(2).build() },
    )

    @Test
    fun nonHelloFirstFrameIsRefused() = assertHandshakeFails(
        envelope { seq = 1; ping = ping { nonce = 1 } },
    )

    @Test
    fun peerClosingBeforeHelloIsAnError() = assertHandshakeFails()

    @Test
    fun aHelloOverTheHandshakeLimitIsRefused() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val big = desktopHello.toBuilder().setDeviceName("x".repeat(FrameCodec.HANDSHAKE_MAX_FRAME_BYTES)).build()
        FrameCodec().write(envelope { seq = 1; hello = big }, desktopEnd.output)
        desktopEnd.output.flush()

        val error = withTimeout(5_000) { runCatching { Session(phoneEnd).handshake(phoneHello) }.exceptionOrNull() }

        assertTrue("got $error", error is SessionException)
        val desktop = FrameCodec()
        assertTrue(desktop.read(desktopEnd.input)!!.hasHello())
        val goodbye = desktop.read(desktopEnd.input)!!.goodbye
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, goodbye.reason)
        assertTrue(goodbye.detail, goodbye.detail.contains("${FrameCodec.HANDSHAKE_MAX_FRAME_BYTES}-byte limit"))
    }

    @Test
    fun largeFramesWaitUntilTheDesktopIsPaired() = runBlocking {
        val large = envelope {
            seq = 2
            notificationPosted = notificationPosted { notification = phoneNotification { text = "x".repeat(100_000) } }
        }
        for (paired in listOf(false, true)) {
            val (phoneEnd, desktopEnd) = connectedPair()
            val desktop = FrameCodec()
            desktop.write(envelope { seq = 1; hello = desktopHello }, desktopEnd.output)
            desktop.write(large, desktopEnd.output)
            desktopEnd.output.flush()
            val phone = Session(phoneEnd)

            val received = withTimeout(5_000) {
                phone.handshake(phoneHello)
                if (paired) phone.markPaired()
                runCatching { phone.next() }
            }

            if (paired) {
                assertEquals(large, received.getOrThrow())
            } else {
                val error = received.exceptionOrNull()
                assertTrue("got $error", error is SessionException)
                assertTrue("got $error", error!!.message!!.contains("${FrameCodec.HANDSHAKE_MAX_FRAME_BYTES}-byte limit"))
            }
        }
    }

    /** A phone session and a desktop session over one connection, Hellos exchanged. */
    private suspend fun establishedPair(): Pair<Session, Session> {
        val (phoneEnd, desktopEnd) = connectedPair()
        val phone = Session(phoneEnd)
        val desktop = Session(desktopEnd)
        withTimeout(5_000) {
            val desktopSaw = async { desktop.handshake(desktopHello) }
            phone.handshake(phoneHello)
            desktopSaw.await()
        }
        return phone to desktop
    }

    private val push = clipboardPush { text = "hi" }

    @Test
    fun aRequestGetsTheAnswerThatNamesIt() = runBlocking {
        val (phone, desktop) = establishedPair()
        // The phone's reader hands the answer over and returns only what nobody waits for.
        val reader = async(Dispatchers.IO) { phone.next() }
        val answer = async { phone.request { clipboardPush = push } }

        val request = withTimeout(5_000) { desktop.next()!! }
        assertTrue(request.expectsReply)
        desktop.send {
            replyTo = request.seq
            ack = Ack.getDefaultInstance()
        }
        desktop.send {
            replyTo = request.seq + 100
            ack = Ack.getDefaultInstance()
        }

        assertEquals(request.seq, withTimeout(5_000) { answer.await() }.replyTo)
        assertEquals(request.seq + 100, withTimeout(5_000) { reader.await() }!!.replyTo)
    }

    @Test
    fun openRequestsGetTheirOwnAnswersInAnyOrder() = runBlocking {
        val (phone, desktop) = establishedPair()
        val reader = async(Dispatchers.IO) { phone.next() }

        val pending = List(3) { phone.sendRequest { clipboardPush = push } }
        val requests = withTimeout(5_000) { List(3) { desktop.next()!! } }
        assertEquals(pending.map { it.seq }, requests.map { it.seq })
        for (request in requests.reversed()) {
            desktop.send {
                replyTo = request.seq
                ack = Ack.getDefaultInstance()
            }
        }

        withTimeout(5_000) { pending.forEach { assertEquals(it.seq, it.await().replyTo) } }
        // None of them reached the reader.
        desktop.close()
        assertNull(withTimeout(5_000) { reader.await() })
    }

    @Test
    fun anAnswerThatCameInTimeCountsWhenAwaitedLate() = runBlocking {
        val (phone, desktop) = establishedPair()
        val reader = async(Dispatchers.IO) { phone.next() }
        val pending = phone.sendRequest(timeoutMillis = 300) { clipboardPush = push }

        val request = withTimeout(5_000) { desktop.next()!! }
        desktop.send {
            replyTo = request.seq
            ack = Ack.getDefaultInstance()
        }
        delay(600)

        assertEquals(request.seq, pending.await().replyTo)
        desktop.close()
        assertNull(withTimeout(5_000) { reader.await() })
    }

    @Test
    fun aRequestWithoutAnAnswerGivesUpAndALateAnswerIsPassedOn() = runBlocking {
        val (phone, desktop) = establishedPair()

        val error = withTimeout(5_000) { runCatching { phone.request(timeoutMillis = 200) { clipboardPush = push } } }.exceptionOrNull()
        assertTrue("got $error", error is RequestTimeoutException)

        val request = withTimeout(5_000) { desktop.next()!! }
        desktop.send {
            replyTo = request.seq
            ack = Ack.getDefaultInstance()
        }
        assertEquals(request.seq, withTimeout(5_000) { phone.next() }!!.replyTo)
    }

    @Test
    fun aRequestFailsAtOnceWhenTheSessionEnds() = runBlocking {
        val (phone, desktop) = establishedPair()
        val answer = async { runCatching { phone.request { clipboardPush = push } } }
        withTimeout(5_000) { desktop.next()!! }

        phone.close()

        val error = withTimeout(5_000) { answer.await() }.exceptionOrNull()
        assertTrue("got $error", error is SessionException)
    }

    @Test
    fun thePeersLatestFeatureStatusIsKeptAndAFeatureIsActiveWhenBothHellosListIt() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        val phone = Session(phoneEnd)
        val desktop = Session(desktopEnd)
        withTimeout(5_000) {
            val desktopSaw = async { desktop.handshake(desktopHello.toBuilder().addFeatures("clipboard").addFeatures("files").build()) }
            phone.handshake(phoneHello)
            desktopSaw.await()
        }
        assertTrue(phone.isActive("clipboard"))
        assertFalse("only the desktop lists it", phone.isActive("files"))
        assertFalse(phone.isReadyOnPeer("clipboard"))

        fun status(vararg states: Pair<String, FeatureAvailability>) = featureStatus {
            states.forEach { (name, availability) ->
                features += featureState {
                    feature = name
                    this.availability = availability
                }
            }
        }
        desktop.send { featureStatus = status("clipboard" to FeatureAvailability.FEATURE_AVAILABILITY_READY) }
        desktop.send { featureStatus = status("files" to FeatureAvailability.FEATURE_AVAILABILITY_DISABLED) }

        assertTrue(withTimeout(5_000) { phone.next()!! }.hasFeatureStatus())
        assertTrue(phone.isReadyOnPeer("clipboard"))
        withTimeout(5_000) { phone.next()!! }
        // Each status replaces the last.
        assertEquals(mapOf("files" to FeatureAvailability.FEATURE_AVAILABILITY_DISABLED), phone.peerFeatures.value)
        assertFalse(phone.isReadyOnPeer("clipboard"))
    }

    @Test
    fun theReaderKeepsReadingWhileAWriteWaitsForThePeer() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair(bufferBytes = 16 * 1024)
        // Ends a run that hangs, as it would if the reader waited for the write below.
        closeLater(desktopEnd, 20_000)
        val phone = Session(phoneEnd)
        val desktop = Session(desktopEnd)
        withTimeout(5_000) {
            val desktopSaw = async { desktop.handshake(desktopHello) }
            phone.handshake(phoneHello)
            desktopSaw.await()
        }
        phone.markPaired()
        desktop.markPaired()
        // 256 KiB, far more than the socket buffers hold.
        val large = clipboardPush { text = "x".repeat(256 * 1024) }

        // The phone writes a large frame the desktop does not read yet.
        val phoneWrite = async(Dispatchers.IO) { phone.send { clipboardPush = large } }
        delay(300)
        assertFalse("the buffers took the whole frame", phoneWrite.isCompleted)

        // The desktop asks for a Pong, then writes a large frame of its own,
        // and reads only once the phone has read that. The phone's reader must
        // answer the Ping without waiting for its writer, and read on.
        val phoneRead = async(Dispatchers.IO) { phone.next() }
        withTimeout(10_000) {
            desktop.send { ping = ping { nonce = 7 } }
            desktop.send { clipboardPush = large }
        }
        assertEquals(large, withTimeout(10_000) { phoneRead.await() }!!.clipboardPush)

        // The desktop reads now: the phone's large frame, then the Pong queued behind it.
        withTimeout(10_000) {
            assertEquals(large, desktop.next()!!.clipboardPush)
            assertEquals(7L, desktop.next()!!.pong.nonce)
            assertEquals(2L, phoneWrite.await())
        }
    }

    @Test
    fun aPostedFrameGoesOutInOrderWithoutWaiting() = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        FrameCodec().write(envelope { seq = 1; hello = desktopHello }, desktopEnd.output)
        desktopEnd.output.flush()
        val phone = Session(phoneEnd)
        withTimeout(5_000) { phone.handshake(phoneHello) }

        val first = phone.send { ping = ping { nonce = 1 } }
        phone.post { ping = ping { nonce = 2 } }
        val third = phone.send { ping = ping { nonce = 3 } }

        // After the phone's Hello: the three in the order they were queued.
        val desktop = FrameCodec()
        val frames = List(4) { desktop.read(desktopEnd.input)!! }.drop(1)
        assertEquals(listOf(1L, 2L, 3L), frames.map { it.ping.nonce })
        assertEquals(listOf(2L, 3L, 4L), frames.map { it.seq })
        assertEquals(listOf(2L, 4L), listOf(first, third))
    }

    @Test
    fun aFrameRefusedForItsSizeIsNotSentAndTheNextOneTakesItsSeq() = runBlocking {
        val (phone, desktop) = establishedPair()
        phone.markPaired()
        desktop.markPaired()
        val reader = async(Dispatchers.IO) { phone.next() }

        // Over the 512 KiB limit for a frame without an image.
        val tooLarge = clipboardPush { text = "x".repeat(600 * 1024) }
        val error = runCatching { phone.sendRequest { clipboardPush = tooLarge } }.exceptionOrNull()
        assertTrue("got $error", error is FrameTooLargeException)

        val pending = phone.sendRequest { clipboardPush = push }
        val request = withTimeout(5_000) { desktop.next()!! }
        assertEquals("the phone's Hello was seq 1", 2L, request.seq)
        assertEquals(pending.seq, request.seq)
        desktop.send {
            replyTo = request.seq
            ack = Ack.getDefaultInstance()
        }
        assertEquals(request.seq, withTimeout(5_000) { pending.await() }.replyTo)
        desktop.close()
        assertNull(withTimeout(5_000) { reader.await() })
    }

    @Test
    fun aPostedAnswerIsBuiltAtOnceSoTheQueueHoldsNothingElse() = runBlocking {
        val (phone, transport) = stuckSession()
        stickWriter(phone, transport)
        var built = false

        phone.post {
            built = true
            ping = ping { nonce = 5 }
        }

        // Built although the writer is stuck: what the answer was made from is not kept.
        assertTrue(built)
    }

    @Test
    fun aPeerThatLeavesTooManyAnswersUnreadIsCutOff() = runBlocking {
        val (byCount, countTransport) = stuckSession()
        stickWriter(byCount, countTransport)
        repeat(Session.MAX_POSTED) { byCount.post { ping = ping { nonce = it.toLong() } } }
        assertTrue(byCount.state.value is SessionState.Established)
        byCount.post { ping = ping { } }
        assertEquals(SessionState.Closed, byCount.state.value)

        val (byBytes, bytesTransport) = stuckSession()
        stickWriter(byBytes, bytesTransport)
        val large: Envelope.Builder.() -> Unit = { errorReply = errorReply { detail = "x".repeat(16 * 1024) } }
        val fit = (Session.MAX_POSTED_BYTES / Envelope.newBuilder().apply(large).build().serializedSize).toInt()
        repeat(fit) { byBytes.post(large) }
        assertTrue(byBytes.state.value is SessionState.Established)
        byBytes.post(large)
        assertEquals(SessionState.Closed, byBytes.state.value)
    }

    @Test
    fun aFailedWriteFailsWhatIsQueuedAndEveryOpenRequest() = runBlocking {
        val (phone, transport) = stuckSession(writeTimeoutMillis = 300)
        val open = phone.sendRequest { clipboardPush = push }
        transport.stick()
        val stuck = async(start = CoroutineStart.UNDISPATCHED) { runCatching { phone.send { ping = ping { nonce = 1 } } }.exceptionOrNull() }
        transport.awaitBlockedWrite()
        val queuedSend = async(start = CoroutineStart.UNDISPATCHED) { runCatching { phone.send { ping = ping { nonce = 2 } } }.exceptionOrNull() }
        val queuedRequest = async(start = CoroutineStart.UNDISPATCHED) { runCatching { phone.sendRequest { clipboardPush = push } }.exceptionOrNull() }

        val errors = withTimeout(5_000) {
            listOf(stuck.await(), queuedSend.await(), queuedRequest.await(), runCatching { open.await() }.exceptionOrNull())
        }

        errors.forEach { assertTrue("got $it", it is SessionException) }
        assertTrue(errors[0]!!.message!!.contains("took longer than 300 ms"))
        assertEquals(SessionState.Closed, phone.state.value)
    }

    @Test
    fun goodbyeClosesWithinItsGraceWhileTheWriterIsStuck() = runBlocking {
        val (phone, transport) = stuckSession()
        stickWriter(phone, transport)

        val started = System.nanoTime()
        phone.goodbye(GoodbyeReason.GOODBYE_REASON_SHUTDOWN)
        val tookMillis = (System.nanoTime() - started) / 1_000_000

        assertTrue("took $tookMillis ms", tookMillis < Session.GOODBYE_GRACE_MILLIS + 1_500)
        assertEquals(SessionState.Closed, phone.state.value)
    }

    @Test
    fun aQueuedSendCancelledBeforeItsTurnIsNotWrittenAndTakesNoSeq() = runBlocking {
        val (phone, transport) = stuckSession()
        transport.stick()
        val first = async(start = CoroutineStart.UNDISPATCHED) { phone.send { ping = ping { nonce = 1 } } }
        transport.awaitBlockedWrite()
        val cancelled = launch(start = CoroutineStart.UNDISPATCHED) { phone.send { ping = ping { nonce = 2 } } }
        val third = async(start = CoroutineStart.UNDISPATCHED) { phone.send { ping = ping { nonce = 3 } } }
        cancelled.cancelAndJoin()
        transport.release()

        assertEquals(withTimeout(5_000) { first.await() } + 1, withTimeout(5_000) { third.await() })
        val pings = transport.envelopes().filter { it.hasPing() }
        assertEquals(listOf(1L, 3L), pings.map { it.ping.nonce })
        assertEquals(pings[0].seq + 1, pings[1].seq)
    }

    @Test
    fun aRequestQueuedRightBehindARefusedOneStillGetsItsAnswer() = runBlocking {
        val (phone, desktop) = establishedPair()
        phone.markPaired()
        desktop.markPaired()
        val reader = async(Dispatchers.IO) { phone.next() }
        val tooLarge = clipboardPush { text = "x".repeat(600 * 1024) }
        // The refused request's caller resumes only once the request queued behind it
        // is out, so its clean-up runs after the other request took the same seq.
        val gate = CountDownLatch(1)
        val callerThread = Executors.newSingleThreadExecutor()
        callerThread.execute { gate.await() }
        val caller = callerThread.asCoroutineDispatcher()
        try {
            val refused = async(caller, start = CoroutineStart.UNDISPATCHED) {
                runCatching { phone.sendRequest { clipboardPush = tooLarge } }.exceptionOrNull()
            }
            val pending = phone.sendRequest { clipboardPush = push }
            val request = withTimeout(5_000) { desktop.next()!! }
            assertEquals(pending.seq, request.seq)

            gate.countDown()
            assertTrue(withTimeout(5_000) { refused.await() } is FrameTooLargeException)
            desktop.send {
                replyTo = request.seq
                ack = Ack.getDefaultInstance()
            }
            assertEquals(request.seq, withTimeout(5_000) { pending.await() }.replyTo)
        } finally {
            gate.countDown()
            caller.close()
        }
        desktop.close()
        assertNull(withTimeout(5_000) { reader.await() })
    }

    private fun assertHandshakeFails(vararg fromDesktop: Envelope) = runBlocking {
        val (phoneEnd, desktopEnd) = connectedPair()
        fromDesktop.forEach { FrameCodec().write(it, desktopEnd.output) }
        desktopEnd.output.flush()
        sockets.last().shutdownOutput() // the desktop end sends nothing more
        val phone = Session(phoneEnd)

        val error = withTimeout(5_000) { runCatching { phone.handshake(phoneHello) }.exceptionOrNull() }

        assertTrue("expected SessionException, got $error", error is SessionException)
        assertEquals(SessionState.Closed, phone.state.value)
    }
}
