package dev.link2linux.android.core.network.pairing

import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.SasCode
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.DesktopPeer.Companion.bytes
import dev.link2linux.android.core.network.tls.PeerKeyPolicy
import dev.link2linux.android.core.network.tls.TestDesktop
import dev.link2linux.android.core.network.tls.TlsServer
import dev.link2linux.android.core.network.tls.asSource
import dev.link2linux.android.core.security.PairingCrypto
import dev.link2linux.android.core.security.PairingCrypto.Role
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.DeviceKind
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.PairingOutcome
import dev.link2linux.protocol.v1.PairingProof
import dev.link2linux.protocol.v1.SasCommit
import dev.link2linux.protocol.v1.SasNonce
import dev.link2linux.protocol.v1.hello
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.ServerSocket
import java.security.SecureRandom
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * The phone's pairing exchanges over a real TLS connection, against a
 * desktop written step by step in the test.
 */
class PhonePairingTest {

    private val phone = TestCertificates.generate("test phone")
    private val desktop = TestDesktop()
    private val secret = PairingSecret.of(ByteArray(16).also(SecureRandom()::nextBytes))
    private val executor = Executors.newCachedThreadPool()
    private val listeners = mutableListOf<ServerSocket>()
    private val pairing = PhonePairing(stepTimeoutMillis = 2_000, userTimeoutMillis = 2_000, resultTimeoutMillis = 2_000)
    private val stored = AtomicInteger()
    private val store: suspend () -> Unit = { stored.incrementAndGet() }

    private val phoneHello = hello {
        protocolVersion = 1
        deviceId = "phone-1"
        deviceName = "Phone"
        kind = DeviceKind.DEVICE_KIND_PHONE
    }

    @After
    fun tearDown() {
        listeners.forEach(ServerSocket::close)
        executor.shutdownNow()
    }

    /**
     * Connects [desktop] to the phone over TLS, exchanges Hellos with
     * [method], then runs [desktopSide] on another thread and [phoneSide]
     * here. Returns the phone's failure, or null when it paired.
     */
    private fun run(
        method: PairingMethod,
        desktopSide: DesktopPeer.() -> Unit,
        phoneSide: suspend (PairingChannel) -> Unit,
    ): PairingFailure? = runBlocking {
        val server = TlsServer(phone.asSource(), PeerKeyPolicy { it == desktop.fingerprint })
        val listener = server.listen(port = 0, address = InetAddress.getLoopbackAddress()).also { listeners += it }
        val desktopDone = executor.submit {
            val peer = DesktopPeer(desktop.connect(listener.localPort), desktop)
            peer.sendHello(method)
            peer.readHello()
            peer.desktopSide()
        }
        val transport = server.secure(listener.accept())
        val session = Session(transport)
        val failure = withTimeout(20_000) {
            session.handshake(phoneHello)
            val channel = PairingChannel(session, transport, transport.peerKey, transport.localKey)
            runCatching { phoneSide(channel) }.exceptionOrNull()
        }
        desktopDone.get(20, TimeUnit.SECONDS)
        session.close()
        when (failure) {
            null -> null
            is PairingFailedException -> failure.failure
            else -> throw failure
        }
    }

    private fun DesktopPeer.sendProof(secret: PairingSecret) = send {
        val mac = PairingCrypto.proof(secret, Role.DESKTOP, ekm(PairingCrypto.QR_EXPORTER_LABEL))
        pairingProof = PairingProof.newBuilder().setMac(bytes(mac)).build()
    }

    private fun DesktopPeer.phoneProofVerifies(message: Envelope?): Boolean =
        message != null && message.hasPairingProof() && PairingCrypto.verifyProof(
            secret, Role.PHONE, ekm(PairingCrypto.QR_EXPORTER_LABEL), message.pairingProof.mac.toByteArray(),
        )

    @Test
    fun qrPairingStoresAfterTheDesktopsAcceptance() {
        val sequence = mutableListOf<String>()
        val failure = run(
            PairingMethod.PAIRING_METHOD_QR_CODE,
            desktopSide = {
                sendProof(secret)
                assertTrue("the phone's proof", phoneProofVerifies(read()))
                assertEquals(PairingOutcome.PAIRING_OUTCOME_ACCEPTED, read()!!.pairingResult.outcome)
                synchronized(sequence) { sequence += "desktop stores" }
                assertEquals(0, stored.get())
                sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED)
                // The connection goes on as a session.
                assertTrue(read()!!.hasFeatureStatus())
            },
            phoneSide = { channel ->
                pairing.pairWithQrCode(channel, secret, { true }) { synchronized(sequence) { sequence += "phone stores" }; store() }
                channel.session.send { featureStatus = dev.link2linux.protocol.v1.FeatureStatus.getDefaultInstance() }
            },
        )
        assertNull(failure)
        assertEquals(1, stored.get())
        assertEquals(listOf("desktop stores", "phone stores"), sequence)
    }

    @Test
    fun qrPairingWithAnotherSecretFailsOnBothSides() {
        val wrong = PairingSecret.of(ByteArray(16))
        val failure = run(
            PairingMethod.PAIRING_METHOD_QR_CODE,
            desktopSide = {
                sendProof(wrong)
                val phoneProof = read()
                assertTrue(phoneProof!!.hasPairingProof())
                assertTrue(!PairingCrypto.verifyProof(wrong, Role.PHONE, ekm(PairingCrypto.QR_EXPORTER_LABEL), phoneProof.pairingProof.mac.toByteArray()))
                assertEquals(PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED, read()!!.pairingResult.outcome)
                assertNull(read())
            },
            phoneSide = { pairing.pairWithQrCode(it, secret, { true }, store) },
        )
        assertEquals(PairingFailure.VERIFICATION_FAILED, failure)
        assertEquals(0, stored.get())
    }

    @Test
    fun theDesktopRejectingThePhonesProofEndsThePairing() {
        val failure = run(
            PairingMethod.PAIRING_METHOD_QR_CODE,
            desktopSide = {
                read() // the phone's proof, which this desktop refuses before sending its own
                sendResult(PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED)
                close()
            },
            phoneSide = { pairing.pairWithQrCode(it, secret, { true }, store) },
        )
        assertEquals(PairingFailure.VERIFICATION_FAILED, failure)
        assertEquals(0, stored.get())
    }

    @Test
    fun aDesktopThatSaysNothingTimesOut() {
        val failure = run(
            PairingMethod.PAIRING_METHOD_QR_CODE,
            desktopSide = {
                read() // the phone's proof
                assertEquals(PairingOutcome.PAIRING_OUTCOME_TIMED_OUT, read()!!.pairingResult.outcome)
            },
            phoneSide = { pairing.pairWithQrCode(it, secret, { true }, store) },
        )
        assertEquals(PairingFailure.TIMED_OUT, failure)
    }

    @Test
    fun withoutKeyingMaterialThePhoneRefusesToPair() {
        val failure = run(
            PairingMethod.PAIRING_METHOD_QR_CODE,
            desktopSide = { assertEquals(PairingOutcome.PAIRING_OUTCOME_UNSUPPORTED, read()!!.pairingResult.outcome) },
            phoneSide = { channel ->
                val noExporter = PairingChannel(channel.session, { _, _, _ -> null }, channel.desktopKey, channel.phoneKey)
                pairing.pairWithQrCode(noExporter, secret, { true }, store)
            },
        )
        assertEquals(PairingFailure.UNSUPPORTED, failure)
    }

    /** The desktop's half of SAS up to the code; returns the code it would show. */
    private fun DesktopPeer.sasUntilCode(): SasCode {
        val commit = read()!!
        assertTrue(commit.hasSasCommit())
        val ekm = ekm(PairingCrypto.SAS_EXPORTER_LABEL)
        val desktopNonce = ByteArray(32).also(SecureRandom()::nextBytes)
        send { sasNonce = SasNonce.newBuilder().setNonce(bytes(desktopNonce)).build() }
        val phoneNonce = read()!!.sasNonce.nonce.toByteArray()
        assertArrayEquals("commitment", commit.sasCommit.commitment.toByteArray(), PairingCrypto.sasCommitment(ekm, phoneNonce))
        return PairingCrypto.sasCode(ekm, desktopNonce, phoneNonce)
    }

    @Test
    fun sasPairingShowsTheSameCodeOnBothSides() {
        val desktopCode = CompletableDeferred<SasCode>()
        val attempts = AtomicInteger()
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                desktopCode.complete(sasUntilCode())
                assertEquals(PairingOutcome.PAIRING_OUTCOME_ACCEPTED, read()!!.pairingResult.outcome)
                sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED)
            },
            phoneSide = { channel ->
                pairing.pairWithSas(channel, { code -> code == desktopCode.await() }, { attempts.incrementAndGet() }, store)
            },
        )
        assertNull(failure)
        assertEquals(1, stored.get())
        assertEquals(1, attempts.get())
    }

    @Test
    fun theUserDecliningTheCodeUsesUpAnAttempt() {
        val attempts = AtomicInteger()
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                sasUntilCode()
                assertEquals(PairingOutcome.PAIRING_OUTCOME_DECLINED, read()!!.pairingResult.outcome)
            },
            phoneSide = { pairing.pairWithSas(it, { false }, { attempts.incrementAndGet() }, store) },
        )
        assertEquals(PairingFailure.DECLINED, failure)
        assertEquals(1, attempts.get())
        assertEquals(0, stored.get())
    }

    @Test
    fun theDesktopDecliningWhileThePhoneWaitsEndsThePairing() {
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                sasUntilCode()
                sendResult(PairingOutcome.PAIRING_OUTCOME_DECLINED)
            },
            phoneSide = { pairing.pairWithSas(it, { CompletableDeferred<Boolean>().await() }, {}, store) },
        )
        assertEquals(PairingFailure.DECLINED, failure)
    }

    @Test
    fun aDroppedConnectionAfterTheRevealStillCountsAsAnAttempt() {
        val attempts = AtomicInteger()
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                sasUntilCode()
                close()
            },
            phoneSide = { pairing.pairWithSas(it, { CompletableDeferred<Boolean>().await() }, { attempts.incrementAndGet() }, store) },
        )
        assertEquals(PairingFailure.CONNECTION_LOST, failure)
        assertEquals(1, attempts.get())
    }

    @Test
    fun anOutOfOrderMessageFailsWithoutRevealingTheNonce() {
        val attempts = AtomicInteger()
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                assertTrue(read()!!.hasSasCommit())
                send { sasCommit = SasCommit.newBuilder().setCommitment(bytes(ByteArray(32))).build() }
                assertEquals(PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED, read()!!.pairingResult.outcome)
                assertNull(read())
            },
            phoneSide = { pairing.pairWithSas(it, { true }, { attempts.incrementAndGet() }, store) },
        )
        assertEquals(PairingFailure.VERIFICATION_FAILED, failure)
        assertEquals(0, attempts.get())
    }

    @Test
    fun aShortDesktopNonceIsRefused() {
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                read()
                send { sasNonce = SasNonce.newBuilder().setNonce(bytes(ByteArray(16))).build() }
                assertEquals(PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED, read()!!.pairingResult.outcome)
            },
            phoneSide = { pairing.pairWithSas(it, { true }, {}, store) },
        )
        assertEquals(PairingFailure.VERIFICATION_FAILED, failure)
    }

    @Test
    fun anEarlyAcceptanceFromTheDesktopIsAProtocolViolation() {
        val failure = run(
            PairingMethod.PAIRING_METHOD_SAS,
            desktopSide = {
                sasUntilCode()
                sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED)
                assertEquals(PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED, read()!!.pairingResult.outcome)
            },
            phoneSide = { pairing.pairWithSas(it, { CompletableDeferred<Boolean>().await() }, {}, store) },
        )
        assertEquals(PairingFailure.VERIFICATION_FAILED, failure)
        assertEquals(0, stored.get())
    }
}
