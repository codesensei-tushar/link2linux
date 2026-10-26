package dev.link2linux.android.feature.pairing

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Scaffold
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.tooling.preview.Preview
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.theme.L2lTheme
import dev.link2linux.android.core.model.Feature

@Composable
fun PairingScreen(
    features: List<Feature>,
    onOpenFeature: (Feature) -> Unit,
    modifier: Modifier = Modifier,
    viewModel: PairingViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    PairingScreen(
        uiState = uiState,
        features = features,
        onOpenFeature = onOpenFeature,
        actions = PairingActions(
            onScan = viewModel::startScan,
            onScanned = viewModel::onQrScanned,
            onClearScanError = viewModel::clearScanError,
            onPairWithoutCamera = viewModel::startSasPairing,
            onConfirmCode = viewModel::confirmCode,
            onConfirmReplace = viewModel::confirmReplace,
            onSetConnection = viewModel::setConnectionEnabled,
            onCancel = viewModel::cancel,
            onDismiss = viewModel::dismiss,
            onUnpair = { viewModel.unpair(it.fingerprint) },
        ),
        modifier = modifier,
    )
}

@Composable
internal fun PairingScreen(
    uiState: PairingUiState,
    features: List<Feature>,
    onOpenFeature: (Feature) -> Unit,
    actions: PairingActions,
    modifier: Modifier = Modifier,
) {
    val screen = uiState.screen
    // Back leaves scanning or stops waiting; on the home screen it closes the app as usual.
    BackHandler(enabled = screen !is PairingScreenState.Home) {
        when (screen) {
            is PairingScreenState.Paired, is PairingScreenState.Failed -> actions.onDismiss()
            else -> actions.onCancel()
        }
    }
    Scaffold(
        modifier = modifier,
        topBar = {
            L2lTopAppBar(
                title = stringResource(R.string.feature_pairing_title),
                onBack = if (screen is PairingScreenState.Home) null else actions.onCancel,
            )
        },
    ) { padding ->
        when (screen) {
            PairingScreenState.Home -> HomeContent(uiState, features, onOpenFeature, actions, Modifier.fillMaxSize(), padding)
            PairingScreenState.Scanning -> ScanContent(uiState.scanError, actions, Modifier.fillMaxSize(), padding)
            else -> AttemptContent(screen, actions, Modifier.fillMaxSize(), padding)
        }
    }
}

/** What the pairing screens can do; the view model provides them. */
internal data class PairingActions(
    val onScan: () -> Unit = {},
    val onScanned: (String) -> Unit = {},
    val onClearScanError: () -> Unit = {},
    val onPairWithoutCamera: () -> Unit = {},
    val onConfirmCode: (Boolean) -> Unit = {},
    val onConfirmReplace: (Boolean) -> Unit = {},
    val onSetConnection: (Boolean) -> Unit = {},
    val onCancel: () -> Unit = {},
    val onDismiss: () -> Unit = {},
    val onUnpair: (dev.link2linux.android.core.model.PairedDevice) -> Unit = {},
)

@Preview
@Composable
private fun HomePreview() {
    L2lTheme(dynamicColor = false) {
        PairingScreen(
            uiState = PairingUiState(emptyList(), PairingScreenState.Home, null, connectionEnabled = true),
            features = Feature.optIn,
            onOpenFeature = {},
            actions = PairingActions(),
        )
    }
}
