package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.protocol.v1.Ack
import dev.link2linux.protocol.v1.BluetoothPairingRequest
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.Envelope.PayloadCase
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.errorReply
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map

/** Asks the user to pair the phone with a desktop over Bluetooth. */
fun interface BluetoothPairingPrompt {
    /**
     * Asks the user to pair with the desktop adapter [name] at [address]
     * (upper case, `XX:XX:XX:XX:XX:XX`), replacing an earlier prompt, and
     * withdraws the prompt after the desktop's pairing window of
     * [CallsSessionHooks.PAIRING_WINDOW_MILLIS].
     */
    fun show(name: String, address: String)
}

/**
 * Connects the calls feature to sessions with paired desktops
 * (docs/protocol.md, "Calls").
 *
 * Calls themselves go over Bluetooth hands-free, which the phone's system
 * runs; the session only helps the user pair the two devices. A
 * `BluetoothPairingRequest` is answered as the feature's state says while
 * it is not `READY` (`UNAVAILABLE` on a phone without telephony or
 * Bluetooth, `DISABLED` while the user has it off), `INVALID_ARGUMENT` when
 * the name or address is outside the schema's forms, and otherwise `Ack`
 * once [prompt] asked the user to pair.
 */
class CallsSessionHooks(
    /** The feature's state on this phone. */
    private val availability: StateFlow<FeatureAvailability>,
    private val prompt: BluetoothPairingPrompt,
) : SessionHooks {

    override fun features(): Set<Feature> = setOf(Feature.CALLS)

    override fun featureStatus(): FeatureStatus = statusOf(availability.value)

    // The current value went out with the session's first status.
    override fun featureStatusChanges(): Flow<FeatureStatus> = availability.drop(1).map(::statusOf)

    override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
        if (!session.isActive(Feature.CALLS.wireName)) return false
        if (envelope.payloadCase != PayloadCase.BLUETOOTH_PAIRING_REQUEST) return false
        // Only a request asks the user; one sent as a one-way event is ignored.
        if (!envelope.expectsReply) return true
        val request = envelope.bluetoothPairingRequest
        val refusal = when (availability.value) {
            FeatureAvailability.FEATURE_AVAILABILITY_READY -> pairingRefusal(request)
            FeatureAvailability.FEATURE_AVAILABILITY_DISABLED -> ErrorCode.ERROR_CODE_DISABLED to "Calls are turned off on the phone"
            else -> ErrorCode.ERROR_CODE_UNAVAILABLE to "The phone cannot take calls over Bluetooth"
        }
        if (refusal == null) prompt.show(request.name, request.address.uppercase())
        // Posted: the reader never waits for a write.
        session.post {
            replyTo = envelope.seq
            if (refusal == null) {
                ack = Ack.getDefaultInstance()
            } else {
                errorReply = errorReply {
                    code = refusal.first
                    detail = refusal.second
                }
            }
        }
        return true
    }

    private fun statusOf(availability: FeatureAvailability): FeatureStatus = featureStatus {
        features += featureState {
            feature = Feature.CALLS.wireName
            this.availability = availability
        }
    }

    companion object {
        /** How long the desktop accepts pairing, and the phone's prompt stays (docs/protocol.md, "Calls"). */
        const val PAIRING_WINDOW_MILLIS: Long = 2 * 60_000

        /** The most `BluetoothPairingRequest.name` holds, in UTF-8 bytes: Bluetooth's limit for a device name. */
        const val MAX_NAME_BYTES = 248

        private val ADDRESS = Regex("[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}")

        /** Why [request] is outside the schema's forms, as an `INVALID_ARGUMENT` answer, or null when it is not. */
        internal fun pairingRefusal(request: BluetoothPairingRequest): Pair<ErrorCode, String>? {
            val nameBytes = request.name.encodeToByteArray().size
            val problem = when {
                nameBytes !in 1..MAX_NAME_BYTES -> "name has $nameBytes bytes"
                !ADDRESS.matches(request.address) -> "address is not six hexadecimal bytes separated by colons"
                else -> return null
            }
            return ErrorCode.ERROR_CODE_INVALID_ARGUMENT to problem
        }
    }
}
