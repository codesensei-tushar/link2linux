package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.core.screen.ScreenEngine
import dev.link2linux.android.core.screen.ScreenPeer
import dev.link2linux.android.core.screen.ScreenReply
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.ScreenEndReason
import dev.link2linux.protocol.v1.ScreenFormat
import dev.link2linux.protocol.v1.ScreenFrame
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.screenEnded
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Connects the screen engine to sessions with paired desktops
 * (docs/protocol.md, "Screen").
 *
 * In a session where the feature is active, the desktop is a [ScreenPeer]
 * of the engine. `ScreenStart`, `ScreenStop` and `ScreenInput` are acted on
 * only as requests, and their answers are posted, as the reader never
 * waits for a write; the stream's formats and frames go out with
 * `Session.send` from the engine's own sender, so the time they take to
 * write shows the engine how much waits. The stream ends with the session,
 * and when the desktop's `FeatureStatus` says the feature is not `READY`
 * there.
 */
class ScreenSessionHooks(private val engine: ScreenEngine) : SessionHooks {

    private val lock = Mutex()
    private val peers = HashMap<Session, SessionPeer>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.SCREEN)

    override fun featureStatus(): FeatureStatus = statusOf(engine.enabled.value)

    // The current value went out with the session's first status.
    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.enabled.drop(1).map(::statusOf)

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        if (!isActive(session)) return
        lock.withLock { peers[session] = SessionPeer(desktop.device.id.value, desktop.device.name, session) }
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        val peer = lock.withLock { peers[session] } ?: return false
        when (envelope.payloadCase) {
            // Only requests are acted on; the same messages sent as one-way events are ignored.
            PayloadCase.SCREEN_START -> if (envelope.expectsReply) {
                engine.start(peer, envelope.screenStart) { reply(session, envelope, it) }
            }
            PayloadCase.SCREEN_STOP -> if (envelope.expectsReply) {
                // Answered once the stream stopped, so no frame of it comes after the Ack.
                engine.stop(peer) { reply(session, envelope, it) }
            }
            PayloadCase.SCREEN_INPUT -> if (envelope.expectsReply) {
                engine.input(peer, envelope.screenInput) { reply(session, envelope, it) }
            }
            PayloadCase.SCREEN_KEY_FRAME_REQUEST -> engine.keyFrameRequested(peer)
            PayloadCase.FEATURE_STATUS -> if (!session.isReadyOnPeer(Feature.SCREEN.wireName)) engine.notReadyOn(peer)
            else -> return false
        }
        return true
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        val ended = lock.withLock { peers.remove(session) } ?: return
        engine.sessionEnded(ended)
    }

    private fun reply(session: Session, request: Envelope, reply: ScreenReply) = session.post {
        replyTo = request.seq
        when (reply) {
            ScreenReply.Done -> ack = Ack.getDefaultInstance()
            is ScreenReply.Refused -> errorReply = errorReply {
                code = reply.code
                detail = reply.detail
            }
        }
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.SCREEN.wireName)

    private fun statusOf(enabled: Boolean): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.SCREEN.wireName
            availability = if (enabled) {
                FeatureAvailability.FEATURE_AVAILABILITY_READY
            } else {
                FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
            }
        }
    }

    /** One session's desktop, as the engine sends it the stream. */
    private class SessionPeer(
        override val desktopId: String,
        override val desktopName: String,
        private val session: Session,
    ) : ScreenPeer {
        override suspend fun format(format: ScreenFormat) {
            session.send { screenFormat = format }
        }

        override suspend fun frame(frame: ScreenFrame) {
            session.send { screenFrame = frame }
        }

        override suspend fun ended(reason: ScreenEndReason) {
            session.send { screenEnded = screenEnded { this.reason = reason } }
        }
    }
}
