package dev.link2linux.android.feature.pairing

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.discovery.DiscoveryStatus
import dev.link2linux.android.core.discovery.QrOfferParser
import dev.link2linux.android.core.discovery.QrParseResult
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PairingFailure
import dev.link2linux.android.core.model.PairingState
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.net.Inet6Address
import java.net.InetSocketAddress
import javax.inject.Inject

/**
 * Drives the pairing screens: the paired computers, scanning a code, and the
 * attempt that follows. The phone never dials; once a code is scanned or
 * "pair without camera" is on, [PairingController] waits for the computer.
 */
@HiltViewModel
class PairingViewModel internal constructor(
    pairedDeviceRepository: PairedDeviceRepository,
    private val controller: PairingController,
    discovery: DiscoveryStatus,
    private val clock: () -> Long,
    ticks: Flow<Unit>,
) : ViewModel() {

    @Inject
    constructor(
        pairedDeviceRepository: PairedDeviceRepository,
        controller: PairingController,
        discovery: DiscoveryStatus,
    ) : this(pairedDeviceRepository, controller, discovery, System::currentTimeMillis, secondTicks())

    private val scanning = MutableStateFlow(false)
    private val scanError = MutableStateFlow<ScanError?>(null)

    /** When the phone began waiting for a scanned computer, for the "type the address" fallback. */
    private var qrWaitStartedAt: Long? = null

    val uiState: StateFlow<PairingUiState> = combine(
        pairedDeviceRepository.pairedDevices,
        controller.state,
        scanning,
        scanError,
        combine(discovery.registeredName, discovery.endpoints, ticks) { name, endpoints, _ -> name to endpoints },
    ) { devices, state, scanning, error, (instanceName, endpoints) ->
        PairingUiState(
            devices = devices,
            screen = screenFor(state, scanning, instanceName, endpoints.map(::formatEndpoint)),
            scanError = error,
        )
    }.stateIn(
        scope = viewModelScope,
        started = SharingStarted.WhileSubscribed(5_000),
        initialValue = PairingUiState(devices = null, screen = PairingScreenState.Home, scanError = null),
    )

    private fun screenFor(
        state: PairingState,
        scanning: Boolean,
        instanceName: String?,
        endpoints: List<String>,
    ): PairingScreenState {
        val now = clock()
        if (state !is PairingState.AwaitingDesktop.Qr) qrWaitStartedAt = null
        return when (state) {
            PairingState.Idle -> if (scanning) PairingScreenState.Scanning else PairingScreenState.Home
            is PairingState.AwaitingDesktop.Qr -> {
                val started = qrWaitStartedAt ?: now.also { qrWaitStartedAt = it }
                PairingScreenState.WaitingForQr(
                    desktopName = state.offer.desktopName,
                    secondsLeft = secondsUntil(state.untilEpochMillis, now),
                    // Nothing connected after 15 seconds: show where to type the phone in on the computer.
                    endpoints = if (now - started >= ADDRESS_HINT_DELAY_MILLIS) endpoints else emptyList(),
                    showAddressHint = now - started >= ADDRESS_HINT_DELAY_MILLIS,
                )
            }
            is PairingState.AwaitingDesktop.Sas -> PairingScreenState.WaitingForSas(
                instanceName = instanceName,
                endpoints = endpoints,
                secondsLeft = secondsUntil(state.untilEpochMillis, now),
                attemptsLeft = state.attemptsLeft,
                lastFailure = state.lastFailure,
            )
            is PairingState.Verifying -> PairingScreenState.Verifying(state.desktopName)
            is PairingState.ConfirmCode -> PairingScreenState.ConfirmCode(state.code.formatted, state.desktopName)
            is PairingState.ConfirmReplace -> PairingScreenState.ConfirmReplace(state.existing.device.name, state.desktopName)
            is PairingState.Paired -> PairingScreenState.Paired(state.device)
            is PairingState.Failed -> PairingScreenState.Failed(state.reason)
        }
    }

    /** The user chose to pair by scanning; the screen asks for the camera. */
    fun startScan() {
        scanError.value = null
        scanning.value = true
    }

    /** The scanner read [text]. Only a valid, current pairing code starts an attempt. */
    fun onQrScanned(text: String) {
        if (controller.state.value != PairingState.Idle) return
        when (val result = QrOfferParser.parse(text)) {
            is QrParseResult.Parsed -> {
                scanError.value = null
                scanning.value = false
                controller.startQrPairing(result.offer)
            }
            QrParseResult.UpdateRequired -> scanError.value = ScanError.UPDATE_THE_APP
            is QrParseResult.Invalid -> scanError.value = ScanError.NOT_A_PAIRING_CODE
        }
    }

    fun clearScanError() {
        scanError.value = null
    }

    /** The user turned on "pair without camera". */
    fun startSasPairing() {
        scanning.value = false
        controller.startSasPairing()
    }

    fun confirmCode(matches: Boolean) = controller.confirmCode(matches)

    fun confirmReplace(replace: Boolean) = controller.confirmReplace(replace)

    /** Leaves scanning, or stops waiting for a computer, or ends an attempt in progress. */
    fun cancel() {
        scanning.value = false
        scanError.value = null
        controller.cancel()
    }

    /** Closes the result of an attempt. */
    fun dismiss() {
        scanning.value = false
        controller.dismiss()
    }

    fun unpair(fingerprint: KeyFingerprint) {
        viewModelScope.launch { controller.unpair(fingerprint) }
    }

    private fun secondsUntil(epochMillis: Long, now: Long): Int =
        ((epochMillis - now + 999) / 1000).toInt().coerceAtLeast(0)

    private fun formatEndpoint(address: InetSocketAddress): String {
        val host = address.address.hostAddress.orEmpty().substringBefore('%')
        return if (address.address is Inet6Address) "[$host]:${address.port}" else "$host:${address.port}"
    }

    internal companion object {
        const val ADDRESS_HINT_DELAY_MILLIS = 15_000L

        fun secondTicks(): Flow<Unit> = flow {
            while (true) {
                emit(Unit)
                delay(1_000)
            }
        }
    }
}

/** Why a scanned code was not accepted. */
enum class ScanError {
    /** The code is for a newer version of the protocol. */
    UPDATE_THE_APP,

    /** The code is not a link2linux pairing code. */
    NOT_A_PAIRING_CODE,
}

data class PairingUiState(
    /** The paired computers, or null until they are loaded. */
    val devices: List<PairedDevice>?,
    val screen: PairingScreenState,
    val scanError: ScanError?,
)

/** What the pairing flow shows. */
sealed interface PairingScreenState {
    /** Paired computers and the ways to pair another. */
    data object Home : PairingScreenState

    /** The camera is looking for a computer's code. */
    data object Scanning : PairingScreenState

    /** The code was read; the phone waits for that computer to connect. */
    data class WaitingForQr(
        val desktopName: String,
        val secondsLeft: Int,
        /** The phone's addresses with its port, shown once the wait has run long. */
        val endpoints: List<String>,
        val showAddressHint: Boolean,
    ) : PairingScreenState

    /** "Pair without camera": the computer finds the phone by the name shown here. */
    data class WaitingForSas(
        val instanceName: String?,
        val endpoints: List<String>,
        val secondsLeft: Int,
        val attemptsLeft: Int,
        val lastFailure: PairingFailure?,
    ) : PairingScreenState

    data class Verifying(val desktopName: String) : PairingScreenState

    /** The user compares [code] with the computer's screen. */
    data class ConfirmCode(val code: String, val desktopName: String) : PairingScreenState

    /** A computer with the same name as a paired one, [existingName], but another key: replace it? */
    data class ConfirmReplace(val existingName: String, val desktopName: String) : PairingScreenState

    data class Paired(val device: PairedDevice) : PairingScreenState

    data class Failed(val reason: PairingFailure) : PairingScreenState
}
