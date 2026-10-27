package dev.link2linux.android.core.ring

import dev.link2linux.protocol.v1.RingEndReason
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * The phone's side of the ring feature (docs/protocol.md, "Ring"): a
 * desktop makes the phone ring so the user can find it.
 *
 * A desktop's request starts the ringtone ([RingPlayer]), the vibration and
 * the notification, for [durationMillis]; a request while the phone rings,
 * from any desktop, starts that period again. Ringing ends when the user
 * stops it on the phone ([stopOnPhone]), a desktop stops it ([stopFromDesktop]),
 * the period is over, the phone cannot go on, or the user turns the feature
 * off. It does not end with a session: a phone that loses its connection
 * while it rings is the one the user is looking for.
 *
 * When ringing ends, each attached desktop that asked for it during that
 * ringing gets `RingEnded` with the reason. Desktops are told apart by id,
 * so a desktop that reconnected while the phone rang is told too.
 */
class RingEngine(
    private val scope: CoroutineScope,
    private val player: RingPlayer,
    private val vibrator: RingVibrator,
    private val notifier: RingNotifier,
    /** Whether the user has the feature on. */
    val enabled: StateFlow<Boolean>,
    private val durationMillis: Long = RING_MILLIS,
) {

    /** One ringing, from its first request until it ends. */
    private class Ringing(val id: Long) {
        /** The desktops that asked for it, by id. */
        val askedBy = HashSet<String>()
        var timeout: Job? = null
    }

    private val lock = Mutex()
    private var ringing: Ringing? = null // guarded by lock
    private var lastId = 0L // guarded by lock
    private val peers = ArrayList<RingPeer>() // guarded by lock

    private val _ringingFor = MutableStateFlow<String?>(null)

    /** The name of the desktop that asked last while the phone rings, or null while it does not. */
    val ringingFor: StateFlow<String?> = _ringingFor.asStateFlow()

    init {
        scope.launch {
            enabled.collect { on -> if (!on) end(RingEndReason.RING_END_REASON_INTERRUPTED) }
        }
    }

    suspend fun attach(peer: RingPeer) = lock.withLock { peers += peer }

    suspend fun detach(peer: RingPeer) = lock.withLock { peers -= peer }

    /**
     * A `RingRequest` from the desktop [desktopId], called [desktopName].
     * [answer] gets the outcome before any `RingEnded` of this ringing can go
     * out, so a session that queues its answer there sends them in order.
     */
    suspend fun ring(desktopId: String, desktopName: String, answer: (RingAnswer) -> Unit) = lock.withLock {
        if (!enabled.value) return@withLock answer(RingAnswer.DISABLED)
        val current = ringing ?: run {
            val id = ++lastId
            if (!player.start { scope.launch { end(RingEndReason.RING_END_REASON_INTERRUPTED, id) } }) {
                return@withLock answer(RingAnswer.UNAVAILABLE)
            }
            vibrator.start()
            Ringing(id).also { ringing = it }
        }
        current.askedBy += desktopId
        notifier.show(desktopName)
        _ringingFor.value = desktopName
        current.timeout?.cancel()
        current.timeout = scope.launch {
            delay(durationMillis)
            end(RingEndReason.RING_END_REASON_TIMED_OUT, current.id)
        }
        answer(RingAnswer.RINGING)
    }

    /** A desktop's `RingStop`; nothing happens when the phone does not ring. */
    suspend fun stopFromDesktop() = end(RingEndReason.RING_END_REASON_STOPPED_FROM_DESKTOP)

    /** The user stopped the ringing on the phone. */
    fun stopOnPhone() {
        scope.launch { end(RingEndReason.RING_END_REASON_STOPPED_ON_PHONE) }
    }

    /** Ends the ringing [id], or any ringing when null, unless it already ended. */
    private suspend fun end(reason: RingEndReason, id: Long? = null) {
        val told = lock.withLock {
            val current = ringing?.takeIf { id == null || it.id == id } ?: return
            ringing = null
            // From the timeout itself this cancels only what is left of it: nothing that suspends.
            current.timeout?.cancel()
            player.stop()
            vibrator.stop()
            notifier.cancel()
            _ringingFor.value = null
            peers.filter { it.desktopId in current.askedBy }
        }
        for (peer in told) {
            scope.launch { peer.ended(reason) }
        }
    }

    companion object {
        /** How long one request rings (docs/protocol.md, "Ring"). */
        const val RING_MILLIS: Long = 60_000
    }
}
