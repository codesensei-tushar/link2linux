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
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.takeWhile
import kotlinx.coroutines.launch
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

    /**
     * The phone's complete `FeatureStatus` after each change, every change in
     * order: a change away from a state and back is two statuses, which the
     * desktop may act on. The session sends each one.
     *
     * Collecting it must subscribe at once, as collecting a `SharedFlow` or
     * `StateFlow` does: the session starts collecting just before its first
     * status, and a change in between must not be lost.
     */
    fun featureStatusChanges(): Flow<FeatureStatus> = emptyFlow()

    /** The session with [desktop] is up; `FeatureStatus` and `PresenceUpdate` went out. */
    suspend fun onEstablished(desktop: PairedDevice, session: Session) = Unit

    /**
     * A message the pairing layer does not handle. Returns whether a feature
     * took it; a request nobody took is answered with `ERROR_CODE_UNSUPPORTED`.
     *
     * This runs on the session's reader, which must never wait for a write:
     * answer with [Session.post], not [Session.send], and hand longer work
     * to another coroutine.
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
 * desktop's messages. A new `FeatureStatus` follows each change.
 *
 * - `KeyRotation` stores the announced key as the record's next key and is
 *   answered with `Ack`, unless the key is this phone's or another paired
 *   desktop's: a desktop cannot claim a key it does not own.
 * - `Goodbye` ends the session; with `GOODBYE_REASON_UNPAIRED` the record is
 *   deleted too.
 * - A pairing message outside pairing is a protocol error.
 *
 * The session lasts only while its pairing does. When no record accepts
 * the desktop's key or holds this pairing's presence key any more (removed
 * on either side, replaced by a pairing with another key, or lost), the
 * session ends with `Goodbye(UNPAIRED)`. A key rotation keeps the presence
 * key and re-pairing with the same key keeps the key, so neither counts:
 * their older session is replaced as usual.
 */
internal class PairedSession(
    private val devices: PairedDeviceRepository,
    private val hooks: SessionHooks,
    /** Runs after this class changed a record, so the TLS policy follows at once. */
    private val onRecordsChanged: suspend () -> Unit = {},
) {

    /**
     * @param key the key the desktop presented on this connection.
     * @param phoneKey the key this phone presented.
     */
    suspend fun run(key: KeyFingerprint, phoneKey: KeyFingerprint, session: Session) = coroutineScope {
        val desktop = devices.find(key) ?: return@coroutineScope session.close()
        val unpaired = launch {
            devices.pairedDevices.first { list -> list.none { it.accepts(key) || it.presenceKey == desktop.presenceKey } }
            session.goodbye(GoodbyeReason.GOODBYE_REASON_UNPAIRED)
        }
        // Subscribed before the first status goes out, so a change in between is not lost.
        val statusUpdates = launch(start = CoroutineStart.UNDISPATCHED) {
            // A failed send means the connection is gone; its reader ends the
            // session. Until then nothing more goes out.
            hooks.featureStatusChanges().takeWhile { sendStatus(session, it) }.collect()
        }
        try {
            session.send { featureStatus = hooks.featureStatus() }
            session.send {
                presenceUpdate = PresenceUpdate.newBuilder().setPresenceKey(bytes(desktop.presenceKey.toByteArray())).build()
            }
            hooks.onEstablished(desktop, session)
            while (true) {
                val envelope = session.next() ?: break
                if (!handle(key, phoneKey, session, envelope)) break
            }
        } finally {
            unpaired.cancel()
            statusUpdates.cancel()
            session.close()
            withContext(NonCancellable) { hooks.onEnded(desktop, session) }
        }
    }

    /** Returns false when the session is over. */
    private suspend fun handle(key: KeyFingerprint, phoneKey: KeyFingerprint, session: Session, envelope: Envelope): Boolean {
        val desktop = devices.find(key) ?: return false // removed on this phone meanwhile
        when (envelope.payloadCase) {
            PayloadCase.KEY_ROTATION -> {
                val refusal = rotationRefusal(desktop, phoneKey, envelope.keyRotation.nextSpkiSha256.toByteArray())
                if (refusal != null) {
                    reply(session, envelope) { errorReply = error(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, refusal) }
                } else {
                    val next = KeyFingerprint.of(envelope.keyRotation.nextSpkiSha256.toByteArray())
                    // Announcing the current key again changes nothing.
                    if (next != desktop.fingerprint) devices.update(desktop.fingerprint) { it.copy(nextFingerprint = next) }
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

    /** Why [desktop] may not announce [next] as its next key, or null if it may. */
    private suspend fun rotationRefusal(desktop: PairedDevice, phoneKey: KeyFingerprint, next: ByteArray): String? {
        if (next.size != KeyFingerprint.SIZE) return "next_spki_sha256 has ${next.size} bytes"
        val key = KeyFingerprint.of(next)
        if (key == phoneKey) return "next_spki_sha256 is this phone's key"
        val taken = devices.pairedDevices.first().any { it.fingerprint != desktop.fingerprint && it.accepts(key) }
        return if (taken) "next_spki_sha256 belongs to another paired device" else null
    }

    private suspend fun sendStatus(session: Session, status: FeatureStatus): Boolean =
        runCatching { session.send { featureStatus = status } }.isSuccess

    // Posted: this runs on the reader, which never waits for a write.
    private fun reply(session: Session, request: Envelope, fill: Envelope.Builder.() -> Unit) =
        session.post {
            replyTo = request.seq
            fill()
        }

    private fun error(code: ErrorCode, detail: String) = ErrorReply.newBuilder().setCode(code).setDetail(detail).build()

    private fun bytes(value: ByteArray) = ByteString.copyFrom(value)
}
