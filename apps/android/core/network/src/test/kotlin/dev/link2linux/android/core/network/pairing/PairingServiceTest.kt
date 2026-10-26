package dev.link2linux.android.core.network.pairing

import com.google.protobuf.ByteString
import dev.link2linux.android.core.data.InMemoryPairedDeviceRepository
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.QrPairingOffer
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.DesktopPeer.Companion.bytes
import dev.link2linux.android.core.network.tls.TestDesktop
import dev.link2linux.android.core.network.tls.asSource
import dev.link2linux.android.core.security.PairingCrypto
import dev.link2linux.android.core.security.PairingCrypto.Role
import dev.link2linux.android.core.testing.TestCertificates
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.Goodbye
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.KeyRotation
import dev.link2linux.protocol.v1.PairingMethod
import dev.link2linux.protocol.v1.PairingOutcome
import dev.link2linux.protocol.v1.PairingProof
import dev.link2linux.protocol.v1.PresenceUpdate
import dev.link2linux.protocol.v1.SasNonce
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.security.SecureRandom

/** The whole phone side, from the listener to the stored pairing, against test desktops over loopback. */
class PairingServiceTest {

    private val phone = TestCertificates.generate("test phone")
    private val devices = InMemoryPairedDeviceRepository()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val secret = PairingSecret.of(ByteArray(16).also(SecureRandom()::nextBytes))
    private val local = object : LocalDeviceRepository {
        override suspend fun localDevice() = Device(DeviceId("phone-1"), "Phone", DeviceKind.PHONE)
        override suspend fun setName(name: String?) = Unit
    }
    private val services = mutableListOf<PairingService>()

    private fun newService(
        listenAddresses: ListenAddresses = ListenAddresses { it.isLoopbackAddress },
        hooks: SessionHooks = SessionHooks.None,
    ) = PairingService(
        identity = phone.asSource(),
        devices = devices,
        localDevice = local,
        scope = scope,
        listenAddresses = listenAddresses,
        hooks = hooks,
        pairing = PhonePairing(stepTimeoutMillis = 2_000, userTimeoutMillis = 5_000, resultTimeoutMillis = 2_000),
        port = 0,
        bindAddress = InetAddress.getLoopbackAddress(),
    ).also { services += it }

    private val service = newService()

    @After
    fun tearDown() {
        runBlocking { services.forEach { it.stop() } }
        scope.cancel()
    }

    private fun offerFor(desktop: TestDesktop, secret: PairingSecret = this.secret) = QrPairingOffer(
        desktopFingerprint = desktop.fingerprint!!,
        secret = secret,
        desktopName = "Test desktop",
        beaconAddresses = listOf(InetSocketAddress(InetAddress.getByName("192.0.2.7"), 27381)),
        bluetoothAddress = "0C:1A:2B:3C:4D:5E",
    )

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    private suspend fun port(on: PairingService = service): Int = on.listeningPort.filterNotNull().first()

    private suspend fun awaitState(predicate: (PairingState) -> Boolean): PairingState =
        withTimeout(10_000) { service.state.first(predicate) }

    /** Connects [desktop] and exchanges Hellos with [method]. */
    private suspend fun dial(
        desktop: TestDesktop,
        method: PairingMethod,
        deviceId: String = DesktopPeer.DEVICE_ID,
        name: String = "Test desktop",
        features: List<String> = emptyList(),
        on: PairingService = service,
    ): DesktopPeer {
        val port = port(on)
        return io {
            DesktopPeer(desktop.connect(port), desktop).apply {
                sendHello(method, deviceId, features, name)
                readHello()
            }
        }
    }

    /** The desktop's side of a QR pairing that both sides accept. */
    private suspend fun DesktopPeer.pairByQrCode(secret: PairingSecret = this@PairingServiceTest.secret) = io {
        send { pairingProof = PairingProof.newBuilder().setMac(bytes(PairingCrypto.proof(secret, Role.DESKTOP, ekm(PairingCrypto.QR_EXPORTER_LABEL)))).build() }
        assertEquals(PayloadCase.PAIRING_PROOF, read()!!.payloadCase)
        val verdict = read()!!
        if (verdict.pairingResult.outcome == PairingOutcome.PAIRING_OUTCOME_ACCEPTED) sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED)
        verdict.pairingResult.outcome
    }

    /** Reads the session start: FeatureStatus, then PresenceUpdate. Returns the presence key. */
    private suspend fun DesktopPeer.readSessionStart(): ByteArray = io {
        assertEquals(PayloadCase.FEATURE_STATUS, read()!!.payloadCase)
        read()!!.presenceUpdate.presenceKey.toByteArray()
    }

    @Test
    fun qrPairingStoresTheDesktopAndContinuesAsASession() = runBlocking {
        val desktop = TestDesktop()
        service.startQrPairing(offerFor(desktop))
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_QR_CODE)

        assertEquals(PairingOutcome.PAIRING_OUTCOME_ACCEPTED, peer.pairByQrCode())
        val presenceKey = peer.readSessionStart()

        val paired = (awaitState { it is PairingState.Paired } as PairingState.Paired).device
        assertEquals(desktop.fingerprint, paired.fingerprint)
        assertEquals(DesktopPeer.DEVICE_ID, paired.device.id.value)
        assertEquals("0C:1A:2B:3C:4D:5E", paired.bluetoothAddress)
        assertEquals(InetSocketAddress(InetAddress.getLoopbackAddress(), 27381), paired.lastKnownAddresses.first())
        assertArrayEquals(paired.presenceKey.toByteArray(), presenceKey)
        assertEquals(listOf(paired), devices.pairedDevices.first())
        withTimeout(5_000) { service.connected.first { desktop.fingerprint in it } }

        // The desktop removes the pairing during the session.
        io { peer.send { goodbye = Goodbye.newBuilder().setReason(GoodbyeReason.GOODBYE_REASON_UNPAIRED).build() } }
        withTimeout(5_000) { service.connected.first { it.isEmpty() } }
        assertTrue(devices.pairedDevices.first().isEmpty())
    }

    @Test
    fun anotherDesktopThanTheScannedOneCannotConnect() = runBlocking {
        val scanned = TestDesktop()
        service.startQrPairing(offerFor(scanned))
        val intruder = TestDesktop()
        val port = port()

        val result = io { runCatching { intruder.connect(port).also { it.outputStream.write(0); it.inputStream.read() } } }

        assertTrue(result.isFailure || result.getOrNull()?.inputStream?.read() == -1)
        assertTrue(service.state.value is PairingState.AwaitingDesktop.Qr)
        assertTrue(devices.pairedDevices.first().isEmpty())
    }

    @Test
    fun aDesktopWithAnotherSecretFailsToPair() = runBlocking {
        val desktop = TestDesktop()
        service.startQrPairing(offerFor(desktop))
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_QR_CODE)

        assertEquals(PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED, peer.pairByQrCode(PairingSecret.of(ByteArray(16))))
        assertEquals(PairingState.Failed(PairingFailure.VERIFICATION_FAILED), awaitState { it is PairingState.Failed })
        assertTrue(devices.pairedDevices.first().isEmpty())
    }

    /** The desktop's half of SAS up to the code. */
    private suspend fun DesktopPeer.sasUntilCode() = io {
        val commit = read()!!.sasCommit.commitment.toByteArray()
        val ekm = ekm(PairingCrypto.SAS_EXPORTER_LABEL)
        val desktopNonce = ByteArray(32).also(SecureRandom()::nextBytes)
        send { sasNonce = SasNonce.newBuilder().setNonce(bytes(desktopNonce)).build() }
        val phoneNonce = read()!!.sasNonce.nonce.toByteArray()
        assertArrayEquals(commit, PairingCrypto.sasCommitment(ekm, phoneNonce))
        PairingCrypto.sasCode(ekm, desktopNonce, phoneNonce)
    }

    @Test
    fun sasPairingWaitsForTheUserToCompareCodes() = runBlocking {
        service.startSasPairing()
        val desktop = TestDesktop()
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_SAS)

        val code = peer.sasUntilCode()
        val shown = awaitState { it is PairingState.ConfirmCode } as PairingState.ConfirmCode
        assertEquals(code, shown.code)
        service.confirmCode(true)

        io {
            assertEquals(PairingOutcome.PAIRING_OUTCOME_ACCEPTED, peer.read()!!.pairingResult.outcome)
            peer.sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED)
        }
        peer.readSessionStart()
        assertEquals(desktop.fingerprint, (awaitState { it is PairingState.Paired } as PairingState.Paired).device.fingerprint)
    }

    @Test
    fun threeRevealedCodesEndSasMode() = runBlocking {
        service.startSasPairing()
        repeat(3) { attempt ->
            val peer = dial(TestDesktop(), PairingMethod.PAIRING_METHOD_SAS)
            peer.sasUntilCode()
            awaitState { it is PairingState.ConfirmCode }
            io { peer.close() }
            if (attempt < 2) {
                val waiting = awaitState { it is PairingState.AwaitingDesktop.Sas } as PairingState.AwaitingDesktop.Sas
                assertEquals(2 - attempt, waiting.attemptsLeft)
            }
        }
        assertEquals(PairingState.Failed(PairingFailure.CONNECTION_LOST), awaitState { it is PairingState.Failed })
        val port = port()
        assertTrue(io { runCatching { TestDesktop().connect(port).also { it.outputStream.write(0); it.inputStream.read() } } }.isFailure)
    }

    private suspend fun pairedRecord(desktop: TestDesktop, id: String = DesktopPeer.DEVICE_ID, on: PairingService = service): PairedDevice {
        val record = PairedDevice(
            device = Device(DeviceId(id), "Test desktop", DeviceKind.DESKTOP),
            fingerprint = desktop.fingerprint!!,
            pairedAtEpochMillis = 0,
            presenceKey = PresenceKey.of(ByteArray(32) { 9 }),
        )
        devices.save(record)
        on.start()
        return record
    }

    @Test
    fun aPairedDesktopReconnectsAndRotatesItsKey() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED)
        assertArrayEquals(ByteArray(32) { 9 }, peer.readSessionStart())

        val next = TestDesktop()
        val request = io {
            peer.send {
                expectsReply = true
                keyRotation = KeyRotation.newBuilder().setNextSpkiSha256(ByteString.copyFrom(next.fingerprint!!.toByteArray())).build()
            }
            peer.read()!!
        }
        assertEquals(PayloadCase.ACK, request.payloadCase)
        assertEquals(next.fingerprint, devices.find(desktop.fingerprint!!)?.nextFingerprint)
        io { peer.close() }

        // The next connection with the new key makes it current.
        val rotated = dial(next, PairingMethod.PAIRING_METHOD_UNSPECIFIED)
        rotated.readSessionStart()
        val record = devices.pairedDevices.first().single()
        assertEquals(next.fingerprint, record.fingerprint)
        assertNull(record.nextFingerprint)
        assertNull(devices.find(desktop.fingerprint))
    }

    @Test
    fun aDesktopClaimingAnotherDeviceIdIsAProtocolError() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED, deviceId = "someone-else")
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, io { peer.read()!!.goodbye.reason })
    }

    @Test
    fun aSessionAnswersUnknownRequestsAndTracksTheBluetoothAddress() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED)
        peer.readSessionStart()

        val answer = io {
            peer.send { presenceUpdate = PresenceUpdate.newBuilder().setBleAddress(bytes(byteArrayOf(0x0c, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e))).build() }
            // A request with a payload this version does not know.
            val request = peer.send { expectsReply = true }
            peer.read()!!.also { assertEquals(request, it.replyTo) }
        }

        assertEquals(ErrorCode.ERROR_CODE_UNSUPPORTED, answer.errorReply.code)
        assertEquals("0C:1A:2B:3C:4D:5E", devices.find(desktop.fingerprint!!)?.bluetoothAddress)
    }

    @Test
    fun aPairingMessageInASessionIsAProtocolError() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED)
        peer.readSessionStart()

        io { peer.sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED) }

        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, io { peer.read()!!.goodbye.reason })
    }

    @Test
    fun aNewSessionReplacesTheOldOneAndUnpairingTellsTheDesktop() = runBlocking {
        val desktop = TestDesktop()
        val record = pairedRecord(desktop)
        val first = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED)
        first.readSessionStart()
        val second = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED)
        second.readSessionStart()
        assertEquals(GoodbyeReason.GOODBYE_REASON_REPLACED, io { first.read()!!.goodbye.reason })

        service.unpair(record.fingerprint)
        assertEquals(GoodbyeReason.GOODBYE_REASON_UNPAIRED, io { second.read()!!.goodbye.reason })
        assertTrue(devices.pairedDevices.first().isEmpty())
    }

    @Test
    fun aPairingDialNobodyExpectsIsRefused() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_QR_CODE)
        assertEquals(PairingOutcome.PAIRING_OUTCOME_NOT_EXPECTED, io { peer.read()!!.pairingResult.outcome })
    }

    @Test
    fun replacingARecordWithTheSameDeviceIdNeedsTheUsersConsent() = runBlocking {
        val old = pairedRecord(TestDesktop())
        val desktop = TestDesktop()
        service.startQrPairing(offerFor(desktop))
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_QR_CODE)
        io {
            peer.send {
                pairingProof = PairingProof.newBuilder()
                    .setMac(bytes(PairingCrypto.proof(secret, Role.DESKTOP, peer.ekm(PairingCrypto.QR_EXPORTER_LABEL)))).build()
            }
            peer.read()
        }

        val question = awaitState { it is PairingState.ConfirmReplace } as PairingState.ConfirmReplace
        assertEquals(old, question.existing)
        service.confirmReplace(true)
        io {
            assertEquals(PairingOutcome.PAIRING_OUTCOME_ACCEPTED, peer.read()!!.pairingResult.outcome)
            peer.sendResult(PairingOutcome.PAIRING_OUTCOME_ACCEPTED)
        }
        awaitState { it is PairingState.Paired }
        assertEquals(listOf(desktop.fingerprint), devices.pairedDevices.first().map { it.fingerprint })
    }

    @Test
    fun cancellingEndsTheAttempt() = runBlocking {
        service.startSasPairing()
        val peer = dial(TestDesktop(), PairingMethod.PAIRING_METHOD_SAS)
        peer.sasUntilCode()
        awaitState { it is PairingState.ConfirmCode }

        service.cancel()

        assertEquals(PairingState.Idle, service.state.value)
        assertTrue(io { peer.read()?.pairingResult?.outcome != PairingOutcome.PAIRING_OUTCOME_ACCEPTED })
        assertTrue(devices.pairedDevices.first().isEmpty())
    }

    @Test
    fun aConnectionThatDidNotArriveOverALocalNetworkIsClosedBeforeTls() = runBlocking {
        val asked = mutableListOf<InetAddress>()
        val offLan = newService(listenAddresses = ListenAddresses { synchronized(asked) { asked += it }; false })
        val desktop = TestDesktop()
        pairedRecord(desktop, on = offLan)
        val port = port(offLan)

        val result = io { runCatching { desktop.connect(port).also { it.outputStream.write(0); it.inputStream.read() } } }

        assertTrue(result.isFailure || result.getOrNull()?.inputStream?.read() == -1)
        assertNull(desktop.seenPhone) // the phone never sent its certificate
        assertEquals(listOf(InetAddress.getLoopbackAddress()), synchronized(asked) { asked.toList() })
    }

    @Test
    fun idleConnectionsFromOneHostCannotKeepAPairedDesktopOut() = runBlocking<Unit> {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val port = port()
        // More stalled connections than the app has IO threads, all from another loopback address.
        val idle = io {
            List(80) {
                runCatching {
                    Socket().apply {
                        bind(InetSocketAddress(InetAddress.getByName("127.0.0.2"), 0))
                        connect(InetSocketAddress(InetAddress.getLoopbackAddress(), port), 1_000)
                    }
                }.getOrNull()
            }
        }
        try {
            withTimeout(5_000) { dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED).readSessionStart() }
        } finally {
            idle.forEach { it?.close() }
        }
    }

    @Test
    fun aReplacedSessionWhoseDesktopStoppedReadingDoesNotHoldUpTheNewOne() = runBlocking {
        val writerStuck = CompletableDeferred<Unit>()
        val hooks = object : SessionHooks {
            @Volatile var sessions = 0

            override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
                if (sessions++ > 0) return
                // The phone writes far more than the desktop (which never reads) and the socket buffers take.
                val big = Hello.newBuilder().setDeviceName("x".repeat(1_000_000)).build()
                scope.launch {
                    runCatching { repeat(256) { session.send { hello = big } } }
                }
                scope.launch {
                    delay(1_000)
                    writerStuck.complete(Unit)
                }
            }
        }
        val phone = newService(hooks = hooks)
        val desktop = TestDesktop()
        pairedRecord(desktop, on = phone)
        val first = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED, on = phone)
        writerStuck.await()

        val second = withTimeout(5_000) {
            dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED, on = phone).also { it.readSessionStart() }
        }

        withTimeout(5_000) { phone.connected.first { desktop.fingerprint in it } }
        io {
            first.close()
            second.close()
        }
    }

    @Test
    fun aHelloWithAMalformedDeviceIdIsAProtocolError() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop, id = "../desk 1")
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED, deviceId = "../desk 1")
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, io { peer.read()!!.goodbye.reason })
    }

    @Test
    fun aHelloWithTenThousandFeaturesIsAProtocolError() = runBlocking {
        val desktop = TestDesktop()
        pairedRecord(desktop)
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_UNSPECIFIED, features = List(10_000) { "" })
        assertEquals(GoodbyeReason.GOODBYE_REASON_PROTOCOL_ERROR, io { peer.read()!!.goodbye.reason })
    }

    @Test
    fun aLongDesktopNameIsCutBeforeItIsShownOrStored() = runBlocking {
        val desktop = TestDesktop()
        service.startQrPairing(offerFor(desktop))
        val peer = dial(desktop, PairingMethod.PAIRING_METHOD_QR_CODE, name = "Desk\u202E" + "y".repeat(4_096))

        assertEquals(PairingOutcome.PAIRING_OUTCOME_ACCEPTED, peer.pairByQrCode())
        val paired = (awaitState { it is PairingState.Paired } as PairingState.Paired).device
        assertEquals("Desk" + "y".repeat(60), paired.device.name)
    }
}
