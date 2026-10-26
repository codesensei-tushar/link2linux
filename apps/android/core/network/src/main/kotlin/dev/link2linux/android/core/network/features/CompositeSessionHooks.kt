package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.featureStatus
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch

/**
 * Several features' hooks as one. The phone's `Hello` lists every feature,
 * and its `FeatureStatus` holds every feature's state, sent complete again
 * after each change of any of them.
 *
 * The desktop's `FeatureStatus` goes to every hook, since each reads its own
 * feature's state from it. Any other message goes to the hooks in order
 * until one takes it, and each takes only its own feature's payloads. The
 * answers to the phone's own requests never get here: the session hands
 * them to the request that waits for them.
 */
class CompositeSessionHooks(private val hooks: List<SessionHooks>) : SessionHooks {

    constructor(vararg hooks: SessionHooks) : this(hooks.toList())

    override fun features(): Set<Feature> = hooks.flatMapTo(mutableSetOf()) { it.features() }

    override fun featureStatus(): FeatureStatus = merged(hooks.map { it.featureStatus() })

    /**
     * After each hook's change, its new part with the other hooks' parts as
     * they last were. Not `merge`, which subscribes to the hooks only once
     * its own coroutines run: here every hook is subscribed before the
     * collector first suspends.
     */
    override fun featureStatusChanges(): Flow<FeatureStatus> = flow {
        coroutineScope {
            val changes = Channel<Pair<Int, FeatureStatus>>(Channel.UNLIMITED)
            hooks.forEachIndexed { i, hook ->
                launch(start = CoroutineStart.UNDISPATCHED) { hook.featureStatusChanges().collect { changes.send(i to it) } }
            }
            val parts = hooks.mapTo(ArrayList()) { it.featureStatus() }
            for ((i, part) in changes) {
                parts[i] = part
                emit(merged(parts))
            }
        }
    }

    override suspend fun onEstablished(desktop: PairedDevice, session: Session) {
        hooks.forEach { it.onEstablished(desktop, session) }
    }

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean =
        if (envelope.payloadCase == PayloadCase.FEATURE_STATUS) {
            hooks.map { it.onMessage(desktop, session, envelope) }.any { it }
        } else {
            hooks.any { it.onMessage(desktop, session, envelope) }
        }

    /** Every hook releases its part of the session, even when another one fails. */
    override suspend fun onEnded(desktop: PairedDevice, session: Session) {
        var failure: Throwable? = null
        for (hook in hooks) {
            try {
                hook.onEnded(desktop, session)
            } catch (e: Throwable) {
                val first = failure
                if (first == null) failure = e else first.addSuppressed(e)
            }
        }
        failure?.let { throw it }
    }

    private fun merged(parts: List<FeatureStatus>): FeatureStatus = featureStatus {
        parts.forEach { features += it.featuresList }
    }
}
