package dev.link2linux.android.feature.pairing

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.link2linux.android.core.designsystem.component.EmptyState
import dev.link2linux.android.core.designsystem.component.FeatureListItem
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.theme.L2lTheme
import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.designsystem.R as DesignR

@Composable
fun PairingScreen(
    features: List<Feature>,
    onOpenFeature: (Feature) -> Unit,
    modifier: Modifier = Modifier,
    viewModel: PairingViewModel = hiltViewModel(),
) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    PairingScreen(uiState = uiState, features = features, onOpenFeature = onOpenFeature, modifier = modifier)
}

@Composable
internal fun PairingScreen(
    uiState: PairingUiState,
    features: List<Feature>,
    onOpenFeature: (Feature) -> Unit,
    modifier: Modifier = Modifier,
) {
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(R.string.feature_pairing_title)) },
    ) { padding ->
        LazyColumn(contentPadding = padding, modifier = Modifier.fillMaxSize()) {
            when (uiState) {
                PairingUiState.Loading -> Unit
                PairingUiState.NotPaired -> item {
                    EmptyState(
                        icon = painterResource(DesignR.drawable.ic_devices),
                        title = stringResource(R.string.feature_pairing_empty_title),
                        body = stringResource(R.string.feature_pairing_empty_body),
                    )
                }
                is PairingUiState.Paired -> items(uiState.devices, key = { it.device.id.value }) { paired ->
                    ListItem(
                        headlineContent = { Text(paired.device.name) },
                        supportingContent = { Text(stringResource(R.string.feature_pairing_paired)) },
                        leadingContent = { Icon(painterResource(DesignR.drawable.ic_devices), contentDescription = null) },
                    )
                }
            }
            if (features.isNotEmpty()) {
                item {
                    Text(
                        text = stringResource(R.string.feature_pairing_features_header),
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.padding(start = 16.dp, top = 24.dp, end = 16.dp, bottom = 8.dp),
                    )
                }
                items(features, key = Feature::wireName) { feature ->
                    FeatureListItem(feature = feature, onClick = { onOpenFeature(feature) })
                }
            }
        }
    }
}

@Preview
@Composable
private fun NotPairedPreview() {
    L2lTheme(dynamicColor = false) {
        PairingScreen(uiState = PairingUiState.NotPaired, features = Feature.optIn, onOpenFeature = {})
    }
}

@Preview
@Composable
private fun PairedPreview() {
    val desk = PairedDevice(
        Device(DeviceId("desk"), name = "Workstation", kind = DeviceKind.DESKTOP),
        KeyFingerprint.of(ByteArray(32)),
        pairedAtEpochMillis = 0,
        presenceKey = PresenceKey.of(ByteArray(32)),
    )
    L2lTheme(dynamicColor = false) {
        PairingScreen(
            uiState = PairingUiState.Paired(listOf(desk)),
            features = Feature.optIn,
            onOpenFeature = {},
        )
    }
}
