package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import java.net.Inet6Address
import java.net.InetSocketAddress

/**
 * Makes the phone findable while the connection service runs: keeps the mDNS
 * registration in step with the pairings and the pairing attempt, and runs
 * the beacon bursts. It reacts to the paired desktops, the pairing state, the
 * listener's port and sessions, and the networks the phone is on.
 *
 * Without a listening port or a network there is no registration and there
 * are no beacons; the service keeps running and discovery resumes when both
 * are back.
 *
 * [scope] must run its coroutines one at a time (a single-threaded
 * dispatcher): the controller keeps plain state that its coroutines share.
 */
class DiscoveryController(
    private val scope: CoroutineScope,
    private val pairedDevices: PairedDeviceRepository,
    private val pairing: PairingController,
    private val connections: DesktopConnections,
    private val localNetworks: LocalNetworks,
    private val advertiser: ServiceAdvertiser,
    private val sender: BeaconSender,
    private val wakeLock: TimedLock,
    private val multicastLock: HeldLock,
    private val announcer: Announcer = Announcer(),
    private val wallClockMillis: () -> Long = System::currentTimeMillis,
    /** Before Android 14 the system's mDNS daemon needs a new registration whenever a network changes. */
    private val reregisterOnNetworkChange: Boolean = false,
) : DiscoveryStatus {
    private val engine = BeaconEngine(
        scope = scope,
        transport = object : BeaconTransport {
            override fun onBurstStart(kind: BurstKind) {
                wakeLock.hold(WAKE_HOLD_MILLIS)
                // Registering again makes Android send fresh announcements.
                scope.launch { lastRegistered?.let { register(it) } }
            }

            override suspend fun send(datagram: ByteArray, unicast: List<InetSocketAddress>, keepAwake: Boolean) {
                if (keepAwake) wakeLock.hold(WAKE_HOLD_MILLIS)
                sender.send(localNetworks.networks.value, datagram, unicast)
            }
        },
        wallClockMillis = wallClockMillis,
    )

    private val _announced = MutableStateFlow<Announcement?>(null)

    /** What the phone currently announces, or null while it announces nothing. */
    val announced: StateFlow<Announcement?> = _announced.asStateFlow()

    override val registeredName: StateFlow<String?> get() = advertiser.registeredName

    override val endpoints: Flow<List<InetSocketAddress>> =
        combine(connections.listeningPort, localNetworks.networks) { port, networks ->
            if (port == null) return@combine emptyList()
            networks.flatMap { it.addresses }
                .map { it.address }
                .filter { !it.isLinkLocalAddress && !it.isLoopbackAddress && !it.isAnyLocalAddress }
                .sortedBy { it is Inet6Address } // IPv4 first: it is what people type
                .map { InetSocketAddress(it, port) }
                .distinct()
        }

    /** The facts that decide what is announced. */
    private data class Facts(
        val devices: List<PairedDevice>,
        val pairing: PairingState,
        val port: Int?,
        val connected: Set<KeyFingerprint>,
        val networks: List<LocalNetwork>,
    )

    private var job: Job? = null
    private var facts: Facts? = null
    private var lastRegistered: Pair<Announcement, Int>? = null
    private var registerJob: Job? = null

    fun start() {
        if (job != null) return
        job = scope.launch {
            launch { follow() }
            launch { rotatePeriodically() }
        }
    }

    suspend fun stop() {
        job?.cancel()
        job = null
        engine.stop()
        registerJob?.cancel()
        multicastLock.release()
        wakeLock.release()
        lastRegistered = null
        facts = null
        _announced.value = null
        advertiser.unregister()
    }

    /** Tells the engine the system reported a desktop nearby: it starts a beacon burst. */
    fun onDesktopNearby() = engine.trigger()

    private suspend fun follow() {
        var knownNetworkIds = emptySet<Long>()
        var knownNetworks = emptyList<LocalNetwork>()
        combine(
            pairedDevices.pairedDevices,
            pairing.state,
            connections.listeningPort,
            connections.connected,
            localNetworks.networks,
        ) { devices, state, port, connected, networks -> Facts(devices, state, port, connected, networks) }
            .distinctUntilChanged()
            .collect { new ->
                val sas = new.pairing is PairingState.AwaitingDesktop.Sas
                val ids = new.networks.map { it.id }.toSet()
                if ((ids - knownNetworkIds).isNotEmpty()) announcer.rotate(sas) // joined a network
                val networksChanged = new.networks != knownNetworks
                knownNetworkIds = ids
                knownNetworks = new.networks
                facts = new
                apply(new, forceRegister = networksChanged && reregisterOnNetworkChange)
            }
    }

    private suspend fun rotatePeriodically() {
        while (true) {
            delay(Announcer.ROTATE_MILLIS)
            val current = facts ?: continue
            announcer.rotate(current.pairing is PairingState.AwaitingDesktop.Sas)
            apply(current, forceRegister = false)
        }
    }

    private fun apply(facts: Facts, forceRegister: Boolean) {
        val inPairing = facts.pairing is PairingState.AwaitingDesktop ||
            facts.pairing is PairingState.Verifying || facts.pairing is PairingState.ConfirmCode ||
            facts.pairing is PairingState.ConfirmReplace
        if (inPairing) multicastLock.acquire() else multicastLock.release()

        val qr = facts.pairing as? PairingState.AwaitingDesktop.Qr
        val ordered = facts.devices.sortedByDescending { it.lastConnectedEpochMillis ?: Long.MIN_VALUE }
        val port = facts.port

        engine.update(
            BeaconInputs(
                port = port,
                networkAvailable = facts.networks.isNotEmpty(),
                desktops = ordered.map { BeaconDesktop(it.fingerprint, it.presenceKey, it.lastKnownAddresses) },
                connected = facts.connected,
                qrOffer = qr?.offer,
                qrUntilMillis = qr?.untilEpochMillis ?: 0,
            ),
        )

        if (port == null || facts.networks.isEmpty()) {
            registerJob?.cancel()
            lastRegistered = null
            _announced.value = null
            registerJob = scope.launch { advertiser.unregister() }
            return
        }
        val announcement = announcer.announcement(
            AnnounceState(
                presenceKeys = ordered.map { it.presenceKey },
                pairingSecret = qr?.offer?.secret,
                sasMode = facts.pairing is PairingState.AwaitingDesktop.Sas,
            ),
        )
        _announced.value = announcement
        val wanted = announcement to port
        if (wanted != lastRegistered || forceRegister) register(wanted)
    }

    private fun register(wanted: Pair<Announcement, Int>) {
        lastRegistered = wanted
        registerJob?.cancel()
        registerJob = scope.launch { advertiser.register(wanted.first, wanted.second) }
    }

    private companion object {
        const val WAKE_HOLD_MILLIS = 8_000L
    }
}
