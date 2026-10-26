package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.clipboard.ClipboardEngine
import dev.link2linux.android.core.clipboard.ClipboardPeer
import dev.link2linux.android.core.clipboard.ClipboardReply
import dev.link2linux.android.core.clipboard.PushAnswer
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.RequestTimeoutException
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.ClipboardPush
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Connects the clipboard engine to sessions with paired desktops.
 *
 * The feature is active in a session when the desktop's `Hello` lists
 * `clipboard`; in a session where it is not, nothing is attached and the
 * desktop's pushes fall through to `ERROR_CODE_UNSUPPORTED`. In a session
 * where it is, the desktop's `FeatureStatus` tells the engine whether the
 * desktop takes pushes now, and the phone's says `DISABLED` until the user
 * turns the feature on.
 */
class ClipboardSessionHooks(
    private val engine: ClipboardEngine,
    /** How long a push waits for the desktop's answer. */
    private val requestTimeoutMillis: Long = Session.REQUEST_TIMEOUT_MILLIS,
) : SessionHooks {

    private val lock = Mutex()
    private val attached = HashMap<Session, SessionPeer>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.CLIPBOARD)

    override fun featureStatus(): FeatureStatus = statusOf(engine.enabled.value)

    // The current value went out with the session's first status.
    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.enabled.drop(1).map(::statusOf)

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        if (!isActive(session)) return
        val peer = SessionPeer(desktop, session, requestTimeoutMillis)
        lock.withLock { attached[session] = peer }
        engine.attach(peer)
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!isActive(session)) return false
        when (envelope.payloadCase) {
            PayloadCase.CLIPBOARD_PUSH -> {
                // Only a request is answered; a push sent as a one-way event is ignored.
                if (envelope.expectsReply) {
                    val reply = engine.receive(desktop.device.id.value, envelope.clipboardPush)
                    // Posted: the reader never waits for a write.
                    session.post {
                        replyTo = envelope.seq
                        when (reply) {
                            ClipboardReply.Ack -> ack = Ack.getDefaultInstance()
                            is ClipboardReply.Error -> errorReply = reply.message
                        }
                    }
                }
            }
            PayloadCase.FEATURE_STATUS -> {
                val peer = lock.withLock { attached[session] } ?: return false
                val availability = session.peerFeatures.value[Feature.CLIPBOARD.wireName]
                engine.onDesktopStatus(peer, availability ?: FeatureAvailability.FEATURE_AVAILABILITY_UNSPECIFIED)
            }
            else -> return false
        }
        return true
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        val ended = lock.withLock { attached.remove(session) } ?: return
        engine.detach(ended)
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.CLIPBOARD.wireName)

    private fun statusOf(enabled: Boolean): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.CLIPBOARD.wireName
            availability = if (enabled) {
                FeatureAvailability.FEATURE_AVAILABILITY_READY
            } else {
                FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
            }
        }
    }

    /** One session's desktop, as the engine pushes to it. */
    private class SessionPeer(
        desktop: PairedDevice,
        private val session: Session,
        private val timeoutMillis: Long,
    ) : ClipboardPeer {
        override val desktopId = desktop.device.id.value
        override val name = desktop.device.name

        override suspend fun push(push: ClipboardPush): PushAnswer {
            val answer = try {
                session.request(timeoutMillis) { clipboardPush = push }
            } catch (_: RequestTimeoutException) {
                return PushAnswer.NoAnswer
            }
            return when (answer.payloadCase) {
                PayloadCase.ACK -> PushAnswer.Ack
                PayloadCase.ERROR_REPLY -> PushAnswer.Error(answer.errorReply.code)
                else -> session.protocolError("ClipboardPush answered with ${answer.payloadCase}")
            }
        }
    }
}
