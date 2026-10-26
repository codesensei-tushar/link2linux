package dev.link2linux.android.core.network.pairing

import com.google.protobuf.ByteString
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ErrorReply
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.GoodbyeReason
import dev.link2linux.protocol.v1.PresenceUpdate
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext

/**
 * Where feature modules plug into a session with a paired desktop. The
 * pairing layer handles the messages about trust itself (`KeyRotation`,
 * `Goodbye`, the desktop's `PresenceUpdate`) and hands everything else on.
 */
interface SessionHooks {

    /** The features this build implements, listed in the phone's `Hello`. */
    fun features(): Set<Feature> = emptySet()

    /** The phone's complete `FeatureStatus`, sent first in every session. */
    fun featureStatus(): FeatureStatus = FeatureStatus.getDefaultInstance()

    /** The session with [desktop] is up; `FeatureStatus` and `PresenceUpdate` went out. */
    suspend fun onEstablished(desktop: PairedDevice, session: Session) = Unit

    /**
     * A message the pairing layer does not handle. Returns whether a feature
     * took it; a request nobody took is answered with `ERROR_CODE_UNSUPPORTED`.
     */
    suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean = false

    /**
     * The session ended, for whatever reason. A newer session with the same
     * desktop may already be running, so compare [session] with the one
     * [onEstablished] got before releasing anything that is shared.
     */
    suspend fun onEnded(desktop: PairedDevice, session: Session) = Unit

    /** No features yet: an empty `FeatureStatus`, and every request is unsupported. */
    object None : SessionHooks
}

/**
 * Runs a session with a paired desktop until it ends: sends the phone's
 * `FeatureStatus` and this desktop's presence key, then serves the
 * desktop's messages.
 *
 * - `KeyRotation` stores the announced key as the record's next key and is
 *   answered with `Ack`.
 * - `Goodbye` ends the session; with `GOODBYE_REASON_UNPAIRED` the record is
 *   deleted too.
 * - A pairing message outside pairing is a protocol error.
 */
internal class PairedSession(
    private val devices: PairedDeviceRepository,
    private val hooks: SessionHooks,
    /** Runs after this class changed a record, so the TLS policy follows at once. */
    private val onRecordsChanged: suspend () -> Unit = {},
) {

    /** @param key the key the desktop presented on this connection. */
    suspend fun run(key: KeyFingerprint, session: Session) {
        val desktop = devices.find(key) ?: return session.close()
        try {
            session.send { featureStatus = hooks.featureStatus() }
            session.send {
                presenceUpdate = PresenceUpdate.newBuilder().setPresenceKey(bytes(desktop.presenceKey.toByteArray())).build()
            }
            hooks.onEstablished(desktop, session)
            while (true) {
                val envelope = session.next() ?: break
                if (!handle(key, session, envelope)) break
            }
        } finally {
            session.close()
            withContext(NonCancellable) { hooks.onEnded(desktop, session) }
        }
    }

    /** Returns false when the session is over. */
    private suspend fun handle(key: KeyFingerprint, session: Session, envelope: Envelope): Boolean {
        val desktop = devices.find(key) ?: return false // removed on this phone meanwhile
        when (envelope.payloadCase) {
            PayloadCase.KEY_ROTATION -> {
                val next = envelope.keyRotation.nextSpkiSha256.toByteArray()
                if (next.size != KeyFingerprint.SIZE) {
                    reply(session, envelope) { errorReply = error(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, "next_spki_sha256 has ${next.size} bytes") }
                } else {
                    devices.update(desktop.fingerprint) { it.copy(nextFingerprint = KeyFingerprint.of(next)) }
                    onRecordsChanged()
                    reply(session, envelope) { ack = Ack.getDefaultInstance() }
                }
            }
            PayloadCase.GOODBYE -> {
                if (envelope.goodbye.reason == GoodbyeReason.GOODBYE_REASON_UNPAIRED) {
                    devices.forget(desktop.fingerprint)
                    onRecordsChanged()
                }
                return false
            }
            PayloadCase.PRESENCE_UPDATE -> {
                val address = envelope.presenceUpdate.bleAddress.toByteArray()
                val formatted = address.takeIf { it.size == 6 }?.joinToString(":") { "%02X".format(it) }
                devices.update(desktop.fingerprint) { it.copy(bluetoothAddress = formatted) }
            }
            PayloadCase.PAIRING_PROOF, PayloadCase.SAS_COMMIT, PayloadCase.SAS_NONCE, PayloadCase.PAIRING_RESULT ->
                session.protocolError("${envelope.payloadCase} outside pairing")
            else -> if (!hooks.onMessage(desktop, session, envelope) && envelope.expectsReply) {
                reply(session, envelope) { errorReply = error(ErrorCode.ERROR_CODE_UNSUPPORTED, "Not supported") }
            }
        }
        return true
    }

    private suspend fun reply(session: Session, request: Envelope, fill: Envelope.Builder.() -> Unit) =
        session.send {
            replyTo = request.seq
            fill()
        }

    private fun error(code: ErrorCode, detail: String) = ErrorReply.newBuilder().setCode(code).setDetail(detail).build()

    private fun bytes(value: ByteArray) = ByteString.copyFrom(value)
}
