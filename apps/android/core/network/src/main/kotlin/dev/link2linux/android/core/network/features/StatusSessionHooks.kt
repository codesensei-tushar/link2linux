package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.android.core.status.StatusEngine
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.PhoneStatus
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.phoneStatusChanged
import dev.link2linux.protocol.v1.phoneStatusReply
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.IOException

/**
 * Connects the phone status engine to sessions with paired desktops
 * (docs/protocol.md, "Phone status").
 *
 * A `PhoneStatusRequest` is answered with the current status and
 * subscribes its session: a follower then sends `PhoneStatusChanged` as the
 * engine paces it, until the session ends, the user turns the feature off,
 * or the desktop's `FeatureStatus` says the feature is not `READY` there.
 * The desktop asks again once it is. A request while the feature is off is
 * answered `DISABLED`, and one in a session where the feature is not active
 * falls through to `UNSUPPORTED`.
 */
class StatusSessionHooks(
    private val engine: StatusEngine,
    /** Where each session's follower runs. */
    private val scope: CoroutineScope,
) : SessionHooks {

    private val lock = Mutex()
    private val followers = HashMap<Session, Job>() // guarded by lock

    override fun features(): Set<Feature> = setOf(Feature.STATUS)

    override fun featureStatus(): FeatureStatus = statusOf(engine.enabled.value)

    // The current value went out with the session's first status.
    override fun featureStatusChanges(): Flow<FeatureStatus> = engine.enabled.drop(1).map(::statusOf)

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!isActive(session)) return false
        when (envelope.payloadCase) {
            PayloadCase.PHONE_STATUS_REQUEST -> when {
                // Only a request subscribes; one sent as a one-way event is ignored.
                !envelope.expectsReply -> Unit
                // Posted: the reader never waits for a write.
                !engine.enabled.value -> session.post {
                    replyTo = envelope.seq
                    errorReply = errorReply {
                        code = ErrorCode.ERROR_CODE_DISABLED
                        detail = "Phone status is turned off on the phone"
                    }
                }
                else -> follow(session, envelope.seq)
            }
            PayloadCase.FEATURE_STATUS -> if (!session.isReadyOnPeer(Feature.STATUS.wireName)) stop(session)
            else -> return false
        }
        return true
    }

    override suspend fun onEnded(desktop: PairedDevice, session: Session) = stop(session)

    /** Answers the request [seq] of [session], then keeps the session current, replacing an earlier follower. */
    private suspend fun follow(session: Session, seq: Long) {
        val follower = scope.launch(start = CoroutineStart.LAZY) {
            try {
                val status = engine.current()
                session.send {
                    replyTo = seq
                    phoneStatusReply = phoneStatusReply { this.status = status }
                }
                engine.follow(status) { changed: PhoneStatus ->
                    session.send { phoneStatusChanged = phoneStatusChanged { this.status = changed } }
                }
            } catch (_: IOException) {
                // The connection failed; the session ends with it.
            }
        }
        lock.withLock { followers.put(session, follower)?.cancel() }
        follower.start()
    }

    private suspend fun stop(session: Session) {
        lock.withLock { followers.remove(session) }?.cancel()
    }

    private fun isActive(session: Session): Boolean = session.isActive(Feature.STATUS.wireName)

    private fun statusOf(enabled: Boolean): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.STATUS.wireName
            availability = if (enabled) {
                FeatureAvailability.FEATURE_AVAILABILITY_READY
            } else {
                FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
            }
        }
    }
}
