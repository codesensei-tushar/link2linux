package dev.link2linux.android.di

import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.data.PairingController
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingState
import dev.link2linux.android.core.model.QrPairingOffer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import javax.inject.Singleton

/**
 * Stands in for the pairing stack (TLS, proofs, the listener) until it lands:
 * it follows the screens' calls but never connects to anything. Replace
 * these two bindings with the real ones and delete this file.
 */
@Module
@InstallIn(SingletonComponent::class)
internal object PlaceholderPairingModule {

    @Provides
    @Singleton
    fun pairingController(devices: PairedDeviceRepository): PairingController = PlaceholderPairingController(devices)

    @Provides
    @Singleton
    fun desktopConnections(): DesktopConnections = object : DesktopConnections {
        override val listeningPort: StateFlow<Int?> = MutableStateFlow(27380)
        override val connected: StateFlow<Set<KeyFingerprint>> = MutableStateFlow(emptySet())
    }
}

private class PlaceholderPairingController(private val devices: PairedDeviceRepository) : PairingController {

    private val _state = MutableStateFlow<PairingState>(PairingState.Idle)
    override val state: StateFlow<PairingState> = _state

    override fun startQrPairing(offer: QrPairingOffer) {
        _state.value = PairingState.AwaitingDesktop.Qr(offer, System.currentTimeMillis() + 5 * 60_000)
    }

    override fun startSasPairing() {
        _state.value = PairingState.AwaitingDesktop.Sas(System.currentTimeMillis() + 2 * 60_000, attemptsLeft = 3)
    }

    override fun confirmCode(matches: Boolean) = Unit

    override fun confirmReplace(replace: Boolean) = Unit

    override fun cancel() {
        _state.value = PairingState.Idle
    }

    override fun dismiss() {
        _state.value = PairingState.Idle
    }

    override suspend fun unpair(fingerprint: KeyFingerprint) = devices.forget(fingerprint)
}
