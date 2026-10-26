package dev.link2linux.android.feature.pairing

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.link2linux.android.core.designsystem.component.EmptyState
import dev.link2linux.android.core.designsystem.component.FeatureListItem
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.designsystem.R as DesignR

@Composable
internal fun HomeContent(
    uiState: PairingUiState,
    features: List<Feature>,
    onOpenFeature: (Feature) -> Unit,
    actions: PairingActions,
    modifier: Modifier,
    padding: PaddingValues,
) {
    var unpairing by remember { mutableStateOf<PairedDevice?>(null) }
    val devices = uiState.devices

    LazyColumn(contentPadding = padding, modifier = modifier) {
        if (devices != null && devices.isEmpty()) {
            item {
                EmptyState(
                    icon = painterResource(DesignR.drawable.ic_devices),
                    title = stringResource(R.string.feature_pairing_empty_title),
                    body = stringResource(R.string.feature_pairing_empty_body),
                )
            }
        }
        if (devices != null) {
            item { PairActions(actions) }
        }
        if (!devices.isNullOrEmpty()) {
            item { SectionHeader(stringResource(R.string.feature_pairing_computers_header)) }
            items(devices, key = { it.fingerprint.toHex() }) { paired ->
                ListItem(
                    headlineContent = { Text(paired.device.name) },
                    supportingContent = { Text(stringResource(R.string.feature_pairing_paired)) },
                    leadingContent = { Icon(painterResource(DesignR.drawable.ic_devices), contentDescription = null) },
                    trailingContent = {
                        TextButton(onClick = { unpairing = paired }) {
                            Text(stringResource(R.string.feature_pairing_unpair))
                        }
                    },
                )
            }
        }
        if (features.isNotEmpty()) {
            item { SectionHeader(stringResource(R.string.feature_pairing_features_header)) }
            items(features, key = Feature::wireName) { feature ->
                FeatureListItem(feature = feature, onClick = { onOpenFeature(feature) })
            }
        }
    }

    unpairing?.let { device ->
        AlertDialog(
            onDismissRequest = { unpairing = null },
            title = { Text(stringResource(R.string.feature_pairing_unpair_title, device.device.name)) },
            text = { Text(stringResource(R.string.feature_pairing_unpair_body)) },
            confirmButton = {
                TextButton(onClick = {
                    actions.onUnpair(device)
                    unpairing = null
                }) { Text(stringResource(R.string.feature_pairing_unpair)) }
            },
            dismissButton = {
                TextButton(onClick = { unpairing = null }) { Text(stringResource(R.string.feature_pairing_cancel)) }
            },
        )
    }
}

@Composable
private fun PairActions(actions: PairingActions) {
    Column(
        verticalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Button(onClick = actions.onScan, modifier = Modifier.fillMaxWidth()) {
            Text(stringResource(R.string.feature_pairing_scan_action))
        }
        OutlinedButton(onClick = actions.onPairWithoutCamera, modifier = Modifier.fillMaxWidth()) {
            Text(stringResource(R.string.feature_pairing_sas_action))
        }
    }
}

@Composable
private fun SectionHeader(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 16.dp, top = 24.dp, end = 16.dp, bottom = 8.dp),
    )
}
