package dev.link2linux.android.core.model

import java.net.InetSocketAddress

/** How a desktop pairs with the phone. Mirrors `link2linux.v1.PairingMethod`. */
enum class PairingMethod {
    /** The phone scanned the desktop's QR code. */
    QR_CODE,

    /** No camera: both screens show a six-digit code that the user compares. */
    SAS,
}

/**
 * What the phone learns from scanning a desktop's QR code
 * (`link2linux://pair?...`). The phone pins [desktopFingerprint] and waits
 * for that desktop to dial it.
 */
data class QrPairingOffer(
    /** `f`: the desktop's key fingerprint. */
    val desktopFingerprint: KeyFingerprint,
    /** `s`: the one-time secret both sides prove they know. */
    val secret: PairingSecret,
    /** `n`: the desktop's name, shown before it connects. */
    val desktopName: String,
    /** `a`: where the phone sends its beacons; the port defaults to 27381. */
    val beaconAddresses: List<InetSocketAddress> = emptyList(),
    /** `b`: the desktop's Bluetooth address as "0C:1A:2B:3C:4D:5E", or null. */
    val bluetoothAddress: String? = null,
)

/** The six-digit code both screens show in SAS pairing. */
@JvmInline
value class SasCode(val value: Int) {
    init {
        require(value in 0..999_999) { "A code has six digits" }
    }

    /** Two groups of three digits, such as "042 917". */
    val formatted: String
        get() = value.toString().padStart(6, '0').let { "${it.substring(0, 3)} ${it.substring(3)}" }
}

/** Why a pairing attempt ended without a pairing. */
enum class PairingFailure {
    /** A proof or commitment did not verify: a wrong QR code, or someone in between. */
    VERIFICATION_FAILED,

    /** The user, on either device, said the codes differ or declined. */
    DECLINED,

    /** Nobody decided in time, or the desktop never connected. */
    TIMED_OUT,

    /** The desktop did not expect to pair with this phone, or the offer expired. */
    NOT_EXPECTED,

    /** One side cannot pair this way, for example because its TLS stack cannot export keying material. */
    UNSUPPORTED,

    /** The connection dropped before both sides had accepted. */
    CONNECTION_LOST,

    /** The phone could not listen for the desktop, or another local error. */
    INTERNAL,
}

/** Where a pairing attempt stands, as the pairing screens show it. */
sealed interface PairingState {

    /** No attempt in progress. */
    data object Idle : PairingState

    /**
     * Waiting for the desktop to dial. Discovery announces the phone while
     * in this state: with the QR code's pairing tag and beacons for
     * [AwaitingDesktop.Qr], and with `pm=1` for [AwaitingDesktop.Sas].
     */
    sealed interface AwaitingDesktop : PairingState {
        /**
         * After a QR scan, until [offer]'s desktop connects or
         * [untilEpochMillis]. Discovery keys the pairing tag and beacons with
         * the offer's secret and sends beacons to its addresses.
         */
        data class Qr(val offer: QrPairingOffer, val untilEpochMillis: Long) : AwaitingDesktop

        /**
         * "Pair without camera" is on until [untilEpochMillis]. Every run in
         * which the phone revealed its nonce uses up an attempt, whatever
         * ended it; after a failed one the phone comes back here with
         * [lastFailure] set while [attemptsLeft] is above zero.
         */
        data class Sas(
            val untilEpochMillis: Long,
            val attemptsLeft: Int,
            val lastFailure: PairingFailure? = null,
        ) : AwaitingDesktop
    }

    /** A desktop connected; the devices are checking each other. */
    data class Verifying(val method: PairingMethod, val desktopName: String) : PairingState

    /** SAS: the user compares [code] with the desktop's screen and answers through `confirmCode`. */
    data class ConfirmCode(val code: SasCode, val desktopName: String) : PairingState

    /** Both devices accepted and the phone stored the pairing. */
    data class Paired(val device: PairedDevice) : PairingState

    /** The attempt ended; nothing was stored. */
    data class Failed(val reason: PairingFailure) : PairingState
}
