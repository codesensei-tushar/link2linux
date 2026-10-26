package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.model.QrPairingOffer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.net.InetSocketAddress

/** A paired desktop, as beacons need it. */
data class BeaconDesktop(
    val fingerprint: KeyFingerprint,
    /** The key this desktop's beacons are made with. */
    val presenceKey: PresenceKey,
    /** Where unicast beacons for this desktop go. */
    val addresses: List<InetSocketAddress>,
)

/** What decides whether beacons go out. */
data class BeaconInputs(
    /** The TCP port the phone listens on, or null while it does not listen. */
    val port: Int? = null,
    /** The phone is on Wi-Fi or Ethernet. */
    val networkAvailable: Boolean = false,
    val desktops: List<BeaconDesktop> = emptyList(),
    /** Desktops with a session right now. */
    val connected: Set<KeyFingerprint> = emptySet(),
    /** The scanned QR code the phone waits on, until [qrUntilMillis] (Unix time). */
    val qrOffer: QrPairingOffer? = null,
    val qrUntilMillis: Long = 0,
)

/** Which kind of burst. */
enum class BurstKind { PAIRING, PRESENCE }

/** Where the engine's beacons and burst starts go; the Android side implements it. */
interface BeaconTransport {

    /** A burst begins: re-register the mDNS service so the desktop hears a fresh announcement. */
    fun onBurstStart(kind: BurstKind)

    /**
     * Sends [datagram] to the broadcast addresses of the phone's networks and,
     * by unicast, to [unicast]. [keepAwake] is true while the burst is in its
     * first minute.
     */
    suspend fun send(datagram: ByteArray, unicast: List<InetSocketAddress>, keepAwake: Boolean)
}

/**
 * Runs the beacon bursts. After a QR scan the phone sends pairing beacons for
 * the scanned offer; with pairings but no session it sends presence beacons,
 * one per desktop without a session at each step. Both kinds run on their own
 * schedules at once when both apply.
 *
 * A burst starts when a condition begins to hold (the service got a port,
 * the phone joined a network, a session was lost, a desktop was paired) or on
 * [trigger] (the system reported the desktop nearby); it stops when the
 * condition ends or its schedule runs out. Anyone in radio range can make
 * the system report the desktop again and again, so [trigger] does nothing
 * within [TRIGGER_GAP_MILLIS] of the last presence burst's start.
 */
class BeaconEngine(
    private val scope: CoroutineScope,
    private val transport: BeaconTransport,
    private val random: RandomBytes = SecureRandomBytes(),
    /**
     * Time since boot, deep sleep included, for the gap between bursts:
     * a clock that stops while the phone sleeps would make a burst hours ago
     * look recent.
     */
    private val elapsedMillis: () -> Long,
    private val wallClockMillis: () -> Long = System::currentTimeMillis,
) {
    private var inputs = BeaconInputs()
    private var pairingJob: Job? = null
    private var pairingSecretKey: Any? = null
    private var presenceJob: Job? = null
    private var presenceStartedAt: Long? = null

    @Synchronized
    fun update(new: BeaconInputs) {
        val old = inputs
        inputs = new
        updatePairing(new)
        updatePresence(old, new)
    }

    /** Starts a new presence burst now, if one is wanted and the last one did not start moments ago. */
    @Synchronized
    fun trigger() {
        val last = presenceStartedAt
        if (last != null && elapsedMillis() - last < TRIGGER_GAP_MILLIS) return
        if (presenceWanted(inputs)) startPresence()
    }

    @Synchronized
    fun stop() {
        pairingJob?.cancel()
        presenceJob?.cancel()
        pairingJob = null
        presenceJob = null
        pairingSecretKey = null
        inputs = BeaconInputs()
    }

    private fun pairingWanted(i: BeaconInputs) =
        i.qrOffer != null && i.port != null && i.networkAvailable && wallClockMillis() < i.qrUntilMillis

    private fun presenceWanted(i: BeaconInputs) =
        i.port != null && i.networkAvailable && i.desktops.any { it.fingerprint !in i.connected }

    private fun updatePairing(new: BeaconInputs) {
        val offer = new.qrOffer
        if (!pairingWanted(new) || offer == null) {
            pairingJob?.cancel()
            pairingJob = null
            pairingSecretKey = null
            return
        }
        // The same offer keeps its burst going, so repeated updates do not restart it.
        val key = Pair(offer.secret, new.port)
        if (pairingJob?.isActive == true && pairingSecretKey == key) return
        pairingSecretKey = key
        pairingJob?.cancel()
        pairingJob = scope.launch {
            transport.onBurstStart(BurstKind.PAIRING)
            runSchedule(BurstSchedule.qrWait) { offset ->
                val current = synchronized(this@BeaconEngine) { inputs }
                val live = current.qrOffer ?: return@runSchedule false
                val port = current.port ?: return@runSchedule false
                if (wallClockMillis() >= current.qrUntilMillis || !current.networkAvailable) return@runSchedule false
                val datagram = Beacon.pairing(live.secret, port, wallClockMillis() / 1000, random.next(Beacon.NONCE_SIZE))
                transport.send(datagram, live.beaconAddresses, BurstSchedule.holdsWakeLock(offset))
                true
            }
        }
    }

    private fun updatePresence(old: BeaconInputs, new: BeaconInputs) {
        if (!presenceWanted(new)) {
            presenceJob?.cancel()
            presenceJob = null
            return
        }
        val becameWanted = !presenceWanted(old)
        val sessionLost = new.desktops.any { d ->
            d.fingerprint !in new.connected && (d.fingerprint in old.connected || old.desktops.none { it.fingerprint == d.fingerprint })
        }
        if (becameWanted || sessionLost) startPresence()
    }

    private fun startPresence() {
        presenceStartedAt = elapsedMillis()
        presenceJob?.cancel()
        presenceJob = scope.launch {
            transport.onBurstStart(BurstKind.PRESENCE)
            runSchedule(BurstSchedule.pairedNoSession) { offset ->
                val current = synchronized(this@BeaconEngine) { inputs }
                val port = current.port
                val waiting = current.desktops.filter { it.fingerprint !in current.connected }
                if (port == null || !current.networkAvailable || waiting.isEmpty()) return@runSchedule false
                for (desktop in waiting) {
                    val datagram = Beacon.presence(
                        desktop.presenceKey, port, wallClockMillis() / 1000, random.next(Beacon.NONCE_SIZE),
                    )
                    transport.send(datagram, desktop.addresses, BurstSchedule.holdsWakeLock(offset))
                }
                true
            }
        }
    }

    companion object {
        /** The shortest time between presence bursts that [trigger] starts. */
        const val TRIGGER_GAP_MILLIS: Long = 60_000
    }

    /** Calls [step] at each offset; stops early when it returns false. */
    private suspend fun runSchedule(offsets: List<Long>, step: suspend (Long) -> Boolean) {
        var elapsed = 0L
        for (offset in offsets) {
            delay(offset - elapsed)
            elapsed = offset
            if (!step(offset)) return
        }
    }
}
