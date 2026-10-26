package dev.link2linux.android.core.network.pairing

import com.google.protobuf.ByteString
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.SasCode
import dev.link2linux.android.core.network.KeyingMaterialExporter
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.security.PairingCrypto
import dev.link2linux.android.core.security.PairingCrypto.Role
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.PairingOutcome
import dev.link2linux.protocol.v1.PairingProof
import dev.link2linux.protocol.v1.PairingResult
import dev.link2linux.protocol.v1.SasCommit
import dev.link2linux.protocol.v1.SasNonce
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withTimeoutOrNull
import java.io.IOException
import java.security.SecureRandom

/** A pairing attempt ended without a pairing; the connection is closed and nothing was stored. */
class PairingFailedException(val failure: PairingFailure, message: String) : IOException(message)

/** The connection a pairing runs on, after the Hello exchange. */
class PairingChannel(
    val session: Session,
    val exporter: KeyingMaterialExporter,
    /** `fp_D`, from this TLS connection. */
    val desktopKey: KeyFingerprint,
    /** `fp_P`, from this TLS connection. */
    val phoneKey: KeyFingerprint,
)

/**
 * Asks the user before the phone accepts. [code] is the SAS code to compare,
 * or null for a QR pairing. Returns whether the user agreed; the phone gives
 * up after the user's time runs out.
 */
fun interface PairingApproval {
    suspend fun approve(code: SasCode?): Boolean
}

/**
 * The phone's side of the pairing exchanges (docs/protocol.md, "Pairing"),
 * run after the Hello exchange on a connection the desktop dialed to pair.
 *
 * Both methods return normally once the desktop's `ACCEPTED` arrived and
 * `store` ran, so the connection can go on as a session. Otherwise they
 * send the right `PairingResult` where one is due, close the connection,
 * and throw [PairingFailedException].
 *
 * `PAIRING_OUTCOME_ACCEPTED` goes in a fixed order: the phone sends it once
 * it has verified the desktop and the user agreed, and stores the pairing
 * only when the desktop's own `ACCEPTED` arrives.
 */
class PhonePairing(
    private val random: SecureRandom = SecureRandom(),
    /** Each step of the exchange other than the user's decision. */
    private val stepTimeoutMillis: Long = 10_000,
    /** The user's time to confirm. */
    private val userTimeoutMillis: Long = 60_000,
    /** How long the phone waits for the desktop's verdict after its own: its user may still be deciding. */
    private val resultTimeoutMillis: Long = 75_000,
) {

    /** QR code pairing: both sides prove they know the scanned code's [secret]. */
    suspend fun pairWithQrCode(channel: PairingChannel, secret: PairingSecret, approval: PairingApproval, store: suspend () -> Unit) {
        val ekm = export(channel, PairingCrypto.QR_EXPORTER_LABEL)
        // Both proofs travel at once; the desktop already holds the secret.
        channel.session.send {
            pairingProof = PairingProof.newBuilder().setMac(bytes(PairingCrypto.proof(secret, Role.PHONE, ekm))).build()
        }
        val message = coroutineScope { awaitStep(channel, readAsync(channel)) }
        if (!message.hasPairingProof()) failOn(channel, message)
        if (!PairingCrypto.verifyProof(secret, Role.DESKTOP, ekm, message.pairingProof.mac.toByteArray())) {
            fail(channel, PairingFailure.VERIFICATION_FAILED, "The desktop's proof did not verify")
        }
        finish(channel, code = null, approval, store)
    }

    /**
     * SAS pairing: commit, exchange nonces, then the user compares codes.
     * [onNonceRevealed] runs just before the phone reveals its nonce; from
     * then on the run counts as an attempt, however it ends.
     */
    suspend fun pairWithSas(
        channel: PairingChannel,
        approval: PairingApproval,
        onNonceRevealed: () -> Unit,
        store: suspend () -> Unit,
    ) {
        val ekm = export(channel, PairingCrypto.SAS_EXPORTER_LABEL)
        val phoneNonce = ByteArray(PairingCrypto.NONCE_SIZE).also(random::nextBytes)
        channel.session.send {
            sasCommit = SasCommit.newBuilder().setCommitment(bytes(PairingCrypto.sasCommitment(ekm, phoneNonce))).build()
        }
        val message = coroutineScope { awaitStep(channel, readAsync(channel)) }
        if (!message.hasSasNonce()) failOn(channel, message)
        val desktopNonce = message.sasNonce.nonce.toByteArray()
        if (desktopNonce.size != PairingCrypto.NONCE_SIZE) {
            fail(channel, PairingFailure.VERIFICATION_FAILED, "The desktop's nonce has ${desktopNonce.size} bytes")
        }
        onNonceRevealed()
        channel.session.send { sasNonce = SasNonce.newBuilder().setNonce(bytes(phoneNonce)).build() }
        finish(channel, PairingCrypto.sasCode(ekm, desktopNonce, phoneNonce), approval, store)
    }

    /** The user's decision, the phone's `ACCEPTED`, the desktop's, then storing. */
    private suspend fun finish(channel: PairingChannel, code: SasCode?, approval: PairingApproval, store: suspend () -> Unit) =
        coroutineScope {
            // The desktop may end the attempt while the user is still deciding.
            val desktop = readAsync(channel)
            val user = async { withTimeoutOrNull(userTimeoutMillis) { approval.approve(code) } }
            val early = select {
                desktop.onAwait { it }
                user.onAwait { null }
            }
            if (early != null) {
                user.cancel()
                failOn(channel, early.getOrElse { lost(channel, it) })
            }
            when (user.await()) {
                null -> fail(channel, PairingFailure.TIMED_OUT, "The user did not decide in time")
                false -> fail(channel, PairingFailure.DECLINED, "The user declined")
                true -> Unit
            }
            channel.session.send { pairingResult = result(PairingOutcome.PAIRING_OUTCOME_ACCEPTED) }
            val answer = withTimeoutOrNull(resultTimeoutMillis) { desktop.await() }
                ?: fail(channel, PairingFailure.TIMED_OUT, "No verdict from the desktop")
            val verdict = answer.getOrElse { lost(channel, it) }
            if (verdict == null || !verdict.hasPairingResult() ||
                verdict.pairingResult.outcome != PairingOutcome.PAIRING_OUTCOME_ACCEPTED
            ) {
                failOn(channel, verdict)
            }
            store()
        }

    private fun CoroutineScope.readAsync(channel: PairingChannel): Deferred<Result<Envelope?>> =
        async { runCatching { channel.session.next() } }

    private suspend fun awaitStep(channel: PairingChannel, read: Deferred<Result<Envelope?>>): Envelope {
        val result = withTimeoutOrNull(stepTimeoutMillis) { read.await() }
            ?: fail(channel, PairingFailure.TIMED_OUT, "The desktop did not answer in time")
        return result.getOrElse { lost(channel, it) } ?: lost(channel, null)
    }

    /** The connection's exporter value for [label]; without one the phone must not pair. */
    private suspend fun export(channel: PairingChannel, label: String): ByteArray =
        channel.exporter.exportKeyingMaterial(
            label,
            PairingCrypto.exporterContext(channel.desktopKey, channel.phoneKey),
            PairingCrypto.EKM_SIZE,
        )?.takeIf { it.size == PairingCrypto.EKM_SIZE }
            ?: fail(channel, PairingFailure.UNSUPPORTED, "Cannot export keying material")

    /**
     * Ends the attempt on an envelope that is not the next step: the
     * desktop's own verdict, or anything out of order, which fails the
     * pairing as unverified.
     */
    private suspend fun failOn(channel: PairingChannel, message: Envelope?): Nothing {
        if (message == null) lost(channel, null)
        if (message.hasPairingResult() && message.pairingResult.outcome != PairingOutcome.PAIRING_OUTCOME_ACCEPTED) {
            channel.session.close()
            throw PairingFailedException(failureFor(message.pairingResult.outcome), "The desktop ended the pairing")
        }
        if (message.hasGoodbye()) lost(channel, null)
        fail(channel, PairingFailure.VERIFICATION_FAILED, "Unexpected ${message.payloadCase} during pairing")
    }

    private suspend fun fail(channel: PairingChannel, failure: PairingFailure, message: String): Nothing {
        sendAndClose(channel, outcomeFor(failure))
        throw PairingFailedException(failure, message)
    }

    private fun lost(channel: PairingChannel, cause: Throwable?): Nothing {
        channel.session.close()
        throw PairingFailedException(PairingFailure.CONNECTION_LOST, "The connection closed: ${cause?.message ?: "end of stream"}")
    }

    private suspend fun sendAndClose(channel: PairingChannel, outcome: PairingOutcome) {
        // Best effort: the connection closes either way.
        runCatching { channel.session.send { pairingResult = result(outcome) } }
        channel.session.close()
    }

    private fun result(outcome: PairingOutcome) = PairingResult.newBuilder().setOutcome(outcome).build()

    private fun bytes(value: ByteArray) = ByteString.copyFrom(value)

    companion object {
        fun failureFor(outcome: PairingOutcome): PairingFailure = when (outcome) {
            PairingOutcome.PAIRING_OUTCOME_DECLINED -> PairingFailure.DECLINED
            PairingOutcome.PAIRING_OUTCOME_NOT_EXPECTED -> PairingFailure.NOT_EXPECTED
            PairingOutcome.PAIRING_OUTCOME_TIMED_OUT -> PairingFailure.TIMED_OUT
            PairingOutcome.PAIRING_OUTCOME_UNSUPPORTED -> PairingFailure.UNSUPPORTED
            else -> PairingFailure.VERIFICATION_FAILED
        }

        fun outcomeFor(failure: PairingFailure): PairingOutcome = when (failure) {
            PairingFailure.DECLINED -> PairingOutcome.PAIRING_OUTCOME_DECLINED
            PairingFailure.TIMED_OUT -> PairingOutcome.PAIRING_OUTCOME_TIMED_OUT
            PairingFailure.NOT_EXPECTED -> PairingOutcome.PAIRING_OUTCOME_NOT_EXPECTED
            PairingFailure.UNSUPPORTED -> PairingOutcome.PAIRING_OUTCOME_UNSUPPORTED
            else -> PairingOutcome.PAIRING_OUTCOME_VERIFICATION_FAILED
        }
    }
}
