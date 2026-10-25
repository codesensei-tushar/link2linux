package dev.link2linux.android.feature.pairing

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.model.PairedDevice
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import javax.inject.Inject

@HiltViewModel
class PairingViewModel @Inject constructor(
    pairedDeviceRepository: PairedDeviceRepository,
) : ViewModel() {

    val uiState: StateFlow<PairingUiState> = pairedDeviceRepository.pairedDevices
        .map { devices -> if (devices.isEmpty()) PairingUiState.NotPaired else PairingUiState.Paired(devices) }
        .stateIn(
            scope = viewModelScope,
            started = SharingStarted.WhileSubscribed(5_000),
            initialValue = PairingUiState.Loading,
        )
}

sealed interface PairingUiState {
    data object Loading : PairingUiState
    data object NotPaired : PairingUiState
    data class Paired(val devices: List<PairedDevice>) : PairingUiState
}
