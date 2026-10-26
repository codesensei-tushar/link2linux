package dev.link2linux.android.core.network.pairing

import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.DesktopListener
import dev.link2linux.android.core.data.ListenAddresses
import dev.link2linux.android.core.data.LocalDeviceRepository
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingMethod
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.QrPairingOffer
import dev.link2linux.android.core.model.SasCode
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.tls.PeerKeyPolicy
import dev.link2linux.android.core.network.tls.TlsServer
import dev.link2linux.android.core.network.tls.TlsTransport
import dev.link2linux.android.core.protocol.toHello
import dev.link2linux.android.core.security.IdentityKeySource
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.DeviceKind as WireDeviceKind
import dev.link2linux.protocol.v1.PairingMethod as WirePairingMethod
import dev.link2linux.protocol.v1.Hello
import dev.link2linux.protocol.v1.PairingOutcome
import dev.link2linux.protocol.v1.PairingResult
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.SecureRandom
import java.util.concurrent.ConcurrentHashMap

/**
 * The phone's listener and everything that arrives on it: pairing
 * connections and sessions with paired desktops. Implements the
 * [PairingController] the pairing screens drive, and [DesktopConnections]
 * and [DesktopListener] for discovery and the connection service.
 *
 * Which desktops may complete TLS is decided by key alone: a paired
 * desktop's current or next key, the key from a scanned QR code, or any key
 * while SAS mode has a free slot. After the Hello exchange, a connection
 * whose Hello names a pairing method pairs; any other must come from a
 * paired desktop whose record (found by key) has the same device id.
 *
 * Before any of that, a connection must come from a host on a local
 * network, the phone's own or one routed to it, to the phone's address on
 * a network it is on ([listenAddresses]), and it must finish the TLS
 * handshake and the Hello
 * exchange in time. Only [MAX_PRE_AUTH]
 * connections may be that far at once, [MAX_PRE_AUTH_PER_SOURCE] of them
 * from one host, and their handshakes share [HANDSHAKE_THREADS] threads of
 * their own, so hosts that open connections and stall cannot use up the
 * app's threads or keep a desktop out.
 */
class PairingService(
    private val identity: IdentityKeySource,
    private val devices: PairedDeviceRepository,
    private val localDevice: LocalDeviceRepository,
    private val scope: CoroutineScope,
    private val listenAddresses: ListenAddresses,
    private val hooks: SessionHooks = SessionHooks.None,
    private val pairing: PhonePairing = PhonePairing(),
    private val random: SecureRandom = SecureRandom(),
    private val now: () -> Long = System::currentTimeMillis,
    private val port: Int = TlsServer.DEFAULT_PORT,
    private val bindAddress: InetAddress? = null,
) : PairingController, DesktopConnections, DesktopListener {

    private val window = PairingWindow(now)
    private val pairedKeys = MutableStateFlow<Set<KeyFingerprint>>(emptySet())
    private val server = TlsServer(identity, PeerKeyPolicy { it in pairedKeys.value || window.acceptsKey(it) })
    private val sessions = ConcurrentHashMap<KeyFingerprint, Live>() // by the record's current key
    private val listenerLock = Mutex()

    // Starting a paired session and removing a pairing take turns, so a
    // session either starts before an unpair, which then ends it, or finds
    // no record.
    private val trust = Mutex()
    private val preAuth = PreAuthLimits(MAX_PRE_AUTH, MAX_PRE_AUTH_PER_SOURCE)

    // A view of the IO pool with threads of its own: blocked handshakes use these, never the app's IO threads.
    private val handshakes = Dispatchers.IO.limitedParallelism(HANDSHAKE_THREADS)
    private val pairedSession = PairedSession(devices, hooks, onRecordsChanged = ::refreshKeys)

    private var listener: ServerSocket? = null
    private var expiry: Job? = null

    @Volatile
    private var pairingConnection: Session? = null

    @Volatile
    private var pendingAnswer: CompletableDeferred<Boolean>? = null

    private val _state = MutableStateFlow<PairingState>(PairingState.Idle)
    override val state: StateFlow<PairingState> = _state.asStateFlow()

    private val _listeningPort = MutableStateFlow<Int?>(null)
    override val listeningPort: StateFlow<Int?> = _listeningPort.asStateFlow()

    private val _connected = MutableStateFlow<Set<KeyFingerprint>>(emptySet())
    override val connected: StateFlow<Set<KeyFingerprint>> = _connected.asStateFlow()

    init {
        scope.launch {
            devices.pairedDevices.collect { list ->
                pairedKeys.value = keysOf(list)
            }
        }
    }

    /**
     * Starts listening, if it is not already. The connection service calls
     * it while the phone has a pairing and is on a local network; starting
     * a pairing calls it too.
     */
    override suspend fun start() {
        listenerLock.withLock {
            if (listener != null) return
            refreshKeys()
            val socket = runInterruptible(Dispatchers.IO) { server.listen(port, bindAddress) }
            listener = socket
            _listeningPort.value = socket.localPort
            scope.launch { acceptLoop(socket) }
        }
    }

    /** Stops listening and ends every session, for example when the phone left the network. */
    override suspend fun stop() {
        listenerLock.withLock {
            listener?.close()
            listener = null
            _listeningPort.value = null
            sessions.values.forEach { scope.launch { it.session.goodbye(GoodbyeReason.GOODBYE_REASON_SHUTDOWN) } }
        }
    }

    override fun startQrPairing(offer: QrPairingOffer) = begin { window.openQr(offer) }

    override fun startSasPairing() = begin { window.openSas() }

    private fun begin(open: () -> PairingState.AwaitingDesktop) {
        abandonAttempt()
        val waiting = open()
        _state.value = waiting
        scheduleExpiry()
        scope.launch {
            runCatching { start() }.onFailure {
                window.close()
                _state.value = PairingState.Failed(PairingFailure.INTERNAL)
            }
        }
    }

    override fun confirmCode(matches: Boolean) {
        if (_state.value is PairingState.ConfirmCode) pendingAnswer?.complete(matches)
    }

    override fun confirmReplace(replace: Boolean) {
        if (_state.value is PairingState.ConfirmReplace) pendingAnswer?.complete(replace)
    }

    override fun cancel() {
        abandonAttempt()
        _state.value = PairingState.Idle
    }

    override fun dismiss() {
        _state.update { if (it is PairingState.Paired || it is PairingState.Failed) PairingState.Idle else it }
    }

    /**
     * Revokes first: the record goes and the TLS policy drops its keys before
     * anything is said, so no new connection with them gets in. Then every
     * session either key opened ends with `Goodbye(UNPAIRED)`.
     */
    override suspend fun unpair(fingerprint: KeyFingerprint) {
        val ended = trust.withLock {
            val record = devices.pairedDevices.first().firstOrNull { it.fingerprint == fingerprint }
            devices.forget(fingerprint)
            refreshKeys()
            val keys = setOfNotNull(fingerprint, record?.nextFingerprint)
            val ending = sessions.entries.filter { (current, live) -> current == fingerprint || live.peerKey in keys }
            ending.forEach { sessions.remove(it.key, it.value) }
            _connected.value = sessions.keys.toSet()
            ending.map { it.value.session }
        }
        ended.forEach { it.goodbye(GoodbyeReason.GOODBYE_REASON_UNPAIRED) }
    }

    private fun abandonAttempt() {
        window.close()
        expiry?.cancel()
        val abandoned = synchronized(this) {
            pendingAnswer?.complete(false)
            pairingConnection.also { pairingConnection = null }
        }
        abandoned?.close()
    }

    private fun scheduleExpiry() {
        expiry?.cancel()
        val until = window.until ?: return
        expiry = scope.launch {
            delay((until - now()).coerceAtLeast(0))
            if (_state.value is PairingState.AwaitingDesktop) {
                window.close()
                _state.value = PairingState.Failed(PairingFailure.TIMED_OUT)
            }
        }
    }

    private suspend fun acceptLoop(socket: ServerSocket) = acceptUntilClosed(socket) { accepted ->
        val slot = admit(accepted)
        if (slot == null) {
            runCatching { accepted.close() }
        } else {
            scope.launch { runCatching { handle(accepted, slot) } }
        }
    }

    /**
     * Whether [socket] may go on to the TLS handshake: it came from a host on
     * a local network, and the caps on unfinished connections leave room for it.
     * Returns the slot it holds until its Hello exchange is over.
     */
    private fun admit(socket: Socket): PreAuthLimits.Slot? {
        if (!listenAddresses.isLocal(socket.localAddress, socket.inetAddress)) return null
        return preAuth.tryAcquire(socket.inetAddress)
    }

    /** One connection, from the TLS handshake to the end of its session. */
    private suspend fun handle(socket: Socket, slot: PreAuthLimits.Slot) {
        val transport: TlsTransport
        val session: Session
        val peer: Hello
        try {
            transport = runInterruptible(handshakes) { server.secure(socket) }
            session = Session(transport)
            val local = localDevice.localDevice()
            peer = session.handshake(local.copy(features = local.features + hooks.features()).toHello())
        } finally {
            slot.release()
        }
        if (peer.kind != WireDeviceKind.DEVICE_KIND_DESKTOP) session.protocolError("Expected a desktop")
        val checked = runCatching { PeerHello.checked(peer) }.getOrElse { session.protocolError(it.message.orEmpty()) }
        dispatch(transport, session, checked)
    }

    private suspend fun dispatch(transport: TlsTransport, session: Session, peer: Hello) {
        when (peer.pairingMethod) {
            WirePairingMethod.PAIRING_METHOD_QR_CODE -> pairByQrCode(transport, session, peer)
            WirePairingMethod.PAIRING_METHOD_SAS -> pairBySas(transport, session, peer)
            else -> reconnect(transport, session, peer)
        }
    }

    /** A paired desktop's normal session. */
    private suspend fun reconnect(transport: TlsTransport, session: Session, peer: Hello) {
        val record = devices.find(transport.peerKey) ?: return session.close() // unpaired since the handshake
        if (record.device.id.value != peer.deviceId) session.protocolError("Hello.device_id does not match the paired desktop")
        devices.update(record.fingerprint) {
            it.copy(
                // The first connection with the announced key makes it current.
                fingerprint = transport.peerKey,
                nextFingerprint = it.nextFingerprint.takeUnless { next -> next == transport.peerKey },
                device = it.device.copy(name = peer.deviceName.ifBlank { it.device.name }),
                lastConnectedEpochMillis = now(),
                lastKnownAddresses = withBeaconAddress(it.lastKnownAddresses, transport.remoteAddress),
            )
        }
        refreshKeys()
        runSession(transport, session)
    }

    private suspend fun pairByQrCode(transport: TlsTransport, session: Session, peer: Hello) {
        val offer = window.qrOfferFor(transport.peerKey) ?: return notExpected(session)
        pairingConnection = session
        _state.value = PairingState.Verifying(PairingMethod.QR_CODE, peer.deviceName)
        val channel = PairingChannel(session, transport, transport.peerKey, transport.localKey)
        val replaced = CompletableDeferred<PairedDevice?>()
        try {
            pairing.pairWithQrCode(channel, offer.secret, approval(session, peer, transport.peerKey, replaced)) {
                store(peer, transport, offer, replaced.await())
            }
        } catch (e: PairingFailedException) {
            if (pairingConnection !== session) return // cancelled or replaced by a newer attempt
            pairingConnection = null
            // A dropped connection leaves the offer open: the desktop dials again.
            val waiting = if (e.failure == PairingFailure.CONNECTION_LOST) window.waitingState() else null
            if (waiting == null) window.close()
            _state.value = waiting ?: PairingState.Failed(e.failure)
            return
        }
        runSession(transport, session)
    }

    private suspend fun pairBySas(transport: TlsTransport, session: Session, peer: Hello) {
        if (!window.claimSas(transport.remoteAddress.address)) return notExpected(session)
        pairingConnection = session
        _state.value = PairingState.Verifying(PairingMethod.SAS, peer.deviceName)
        val channel = PairingChannel(session, transport, transport.peerKey, transport.localKey)
        val replaced = CompletableDeferred<PairedDevice?>()
        try {
            pairing.pairWithSas(channel, approval(session, peer, transport.peerKey, replaced), window::sasAttemptUsed) {
                store(peer, transport, offer = null, replaced.await())
            }
        } catch (e: PairingFailedException) {
            if (pairingConnection !== session) return
            pairingConnection = null
            _state.value = window.sasFailed(e.failure)
            return
        }
        runSession(transport, session)
    }

    /**
     * Asks the user what the protocol requires before the phone accepts: the
     * SAS code, then whether to replace a record that has the desktop's
     * device id but another key. Completes [replaced] with that record when
     * the user agreed to replace it.
     */
    // An object rather than a SAM lambda: lint's opt-in detector crashes on
    // lambdas for suspend fun interfaces.
    private fun approval(
        session: Session,
        peer: Hello,
        key: KeyFingerprint,
        replaced: CompletableDeferred<PairedDevice?>,
    ): PairingApproval = object : PairingApproval {
        override suspend fun approve(code: SasCode?): Boolean {
            if (code != null && !ask(session, PairingState.ConfirmCode(code, peer.deviceName))) return false
            val existing = devices.pairedDevices.first()
                .firstOrNull { it.device.id.value == peer.deviceId && !it.accepts(key) }
            if (existing != null && !ask(session, PairingState.ConfirmReplace(existing, peer.deviceName))) return false
            if (pairingConnection !== session) return false
            _state.value = PairingState.Verifying(if (code == null) PairingMethod.QR_CODE else PairingMethod.SAS, peer.deviceName)
            replaced.complete(existing)
            return true
        }
    }

    /** Shows [question] and waits for the answer, unless [session] is no longer the current attempt. */
    private suspend fun ask(session: Session, question: PairingState): Boolean {
        val answer = CompletableDeferred<Boolean>()
        synchronized(this) {
            if (pairingConnection !== session) return false
            pendingAnswer = answer
            _state.value = question
        }
        return try {
            answer.await()
        } finally {
            pendingAnswer = null
        }
    }

    /** Runs when the desktop's `ACCEPTED` arrived: the phone stores last. */
    private suspend fun store(peer: Hello, transport: TlsTransport, offer: QrPairingOffer?, replaced: PairedDevice?) {
        val record = PairedDevice(
            device = Device(DeviceId(peer.deviceId), peer.deviceName.ifBlank { peer.deviceId }, DeviceKind.DESKTOP),
            fingerprint = transport.peerKey,
            pairedAtEpochMillis = now(),
            presenceKey = PresenceKey.of(ByteArray(PresenceKey.SIZE).also(random::nextBytes)),
            lastConnectedEpochMillis = now(),
            bluetoothAddress = offer?.bluetoothAddress,
            lastKnownAddresses = withBeaconAddress(offer?.beaconAddresses.orEmpty(), transport.remoteAddress),
        )
        if (replaced != null) devices.forget(replaced.fingerprint)
        devices.save(record)
        refreshKeys()
        expiry?.cancel()
        window.close()
        pairingConnection = null
        _state.value = PairingState.Paired(record)
    }

    /** A connection from a paired desktop becomes its session, unless its pairing went meanwhile. */
    private suspend fun runSession(transport: TlsTransport, session: Session) {
        session.markPaired()
        val key = transport.peerKey
        val live = Live(session, key)
        val record = trust.withLock {
            val record = devices.find(key) ?: return session.close()
            // One session per desktop: a newer connection replaces an older one.
            sessions.put(record.fingerprint, live)?.let { replaced(it.session) }
            _connected.value = sessions.keys.toSet()
            record
        }
        try {
            pairedSession.run(key, transport.localKey, session)
        } finally {
            sessions.remove(record.fingerprint, live)
            _connected.value = sessions.keys.toSet()
        }
    }

    /**
     * Ends [old], which a newer session with the same desktop replaced, off
     * the new session's path: the old connection may be half-open or its
     * peer may have stopped reading, so its `Goodbye` gets
     * [REPLACED_GOODBYE_MILLIS] before the connection is closed anyway.
     */
    private fun replaced(old: Session) {
        scope.launch {
            val cutOff = launch {
                delay(REPLACED_GOODBYE_MILLIS)
                old.close()
            }
            old.goodbye(GoodbyeReason.GOODBYE_REASON_REPLACED)
            cutOff.cancel()
        }
    }

    private suspend fun notExpected(session: Session) {
        runCatching {
            session.send { pairingResult = PairingResult.newBuilder().setOutcome(PairingOutcome.PAIRING_OUTCOME_NOT_EXPECTED).build() }
        }
        session.close()
    }

    /** The desktop's address with the beacon port first, then the others, at most [MAX_ADDRESSES]. */
    private fun withBeaconAddress(known: List<InetSocketAddress>, remote: InetSocketAddress): List<InetSocketAddress> {
        val beacon = InetSocketAddress(remote.address, BEACON_PORT)
        return (listOf(beacon) + known.filterNot { it == beacon }).take(MAX_ADDRESSES)
    }

    /** Updates the keys the TLS policy accepts right away, without waiting for the repository flow. */
    private suspend fun refreshKeys() {
        pairedKeys.value = keysOf(devices.pairedDevices.first())
    }

    private fun keysOf(list: List<PairedDevice>): Set<KeyFingerprint> =
        list.flatMapTo(mutableSetOf()) { listOfNotNull(it.fingerprint, it.nextFingerprint) }

    /** A running session, with the key the desktop presented for it. */
    private class Live(val session: Session, val peerKey: KeyFingerprint)

    companion object {
        /** Connections that may be in their TLS handshake or Hello exchange at once. */
        const val MAX_PRE_AUTH: Int = 8

        /** Of those, how many may come from one host (an IPv4 address or an IPv6 /64). */
        const val MAX_PRE_AUTH_PER_SOURCE: Int = 2

        /** Threads that run TLS handshakes. */
        const val HANDSHAKE_THREADS: Int = 4

        /** How long a replaced session's `Goodbye` may take before its connection is closed. */
        const val REPLACED_GOODBYE_MILLIS: Long = 2_000

        private const val BEACON_PORT = 27381
        private const val MAX_ADDRESSES = 4
    }
}
