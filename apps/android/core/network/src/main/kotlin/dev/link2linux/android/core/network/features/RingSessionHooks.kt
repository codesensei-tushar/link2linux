package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.core.ring.RingAnswer
import dev.link2linux.android.core.ring.RingEngine
import dev.link2linux.android.core.ring.RingPeer
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.RingEndReason
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.ringEnded
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.IOException

/**
 * Connects the ring engine to sessions with paired desktops
 * (docs/protocol.md, "Ring").
 *
 * In a session where the feature is active, the desktop is attached to the
 * engine, which tells it with `RingEnded` when a ringing it asked for ends.
 * `RingRequest` is answered `Ack` once the phone rings, `DISABLED` while the
 * user has the feature off, and `UNAVAILABLE` when the phone cannot ring,
 * as during a call. `RingStop` is always answered `Ack`, before the
 * `RingEnded` it causes. The session ending does not stop the ringing.
 */
class RingSessionHooks(private val engine: RingEngine) : SessionHooks {

    private val lock = Mutex()
    private val attached = HashMap<Session, SessionPeer>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.RING)

    override fun featureStatus(): FeatureStatus = statusOf(engine.enabled.value)

    // The current value went out with the session's first status.
    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.enabled.drop(1).map(::statusOf)

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        if (!isActive(session)) return
        val peer = SessionPeer(desktop.device.id.value, session)
        lock.withLock { attached[session] = peer }
        engine.attach(peer)
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!isActive(session)) return false
        when (envelope.payloadCase) {
            // Only requests are acted on; the same messages sent as one-way events are ignored.
            PayloadCase.RING_REQUEST -> if (envelope.expectsReply) {
                // Posted, as the reader never waits for a write, and while the engine answers, so the
                // answer goes out before the RingEnded of the ringing it started.
                engine.ring(desktop.device.id.value, desktop.device.name) { answer -> reply(session, envelope, answer) }
            }
            PayloadCase.RING_STOP -> if (envelope.expectsReply) {
                // Queued first, so this desktop gets the Ack before the RingEnded the stop causes.
                session.post {
                    replyTo = envelope.seq
                    ack = Ack.getDefaultInstance()
                }
                engine.stopFromDesktop()
            }
            else -> return false
        }
        return true
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        val ended = lock.withLock { attached.remove(session) } ?: return
        engine.detach(ended)
    }

    private fun reply(session: Session, request: Envelope, answer: RingAnswer) = session.post {
        replyTo = request.seq
        when (answer) {
            RingAnswer.RINGING -> ack = Ack.getDefaultInstance()
            RingAnswer.DISABLED -> errorReply = errorReply {
                code = ErrorCode.ERROR_CODE_DISABLED
                detail = "Ringing is turned off on the phone"
            }
            RingAnswer.UNAVAILABLE -> errorReply = errorReply {
                code = ErrorCode.ERROR_CODE_UNAVAILABLE
                detail = "The phone cannot ring now"
            }
        }
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.RING.wireName)

    private fun statusOf(enabled: Boolean): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.RING.wireName
            availability = if (enabled) {
                FeatureAvailability.FEATURE_AVAILABILITY_READY
            } else {
                FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
            }
        }
    }

    /** One session's desktop, as the engine tells it that ringing ended. */
    private class SessionPeer(override val desktopId: String, private val session: Session) : RingPeer {
        override suspend fun ended(reason: RingEndReason) {
            try {
                session.send { ringEnded = ringEnded { this.reason = reason } }
            } catch (_: IOException) {
                // The connection failed; the session ends with it, and the desktop stops showing the ringing.
            }
        }
    }
}
