package dev.link2linux.android.feature.calls

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.designsystem.component.L2lCallout
import dev.link2linux.android.core.designsystem.component.L2lDivider
import dev.link2linux.android.core.designsystem.component.L2lListItem
import dev.link2linux.android.core.designsystem.component.L2lSection
import dev.link2linux.android.core.designsystem.component.L2lSectionHeader
import dev.link2linux.android.core.designsystem.component.L2lTextButton
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.StatusBadge
import dev.link2linux.android.core.designsystem.component.Tone
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.designsystem.R as DesignR
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject

/** The calls feature for its screen. */
@HiltViewModel
class CallsViewModel @Inject internal constructor(private val runtime: CallsRuntime) : ViewModel() {
    val enabled: StateFlow<Boolean> = runtime.enabled
    val support: CallsSupport = runtime.support

    private val _canPairDirectly = MutableStateFlow(runtime.canPairDirectly())

    /** Whether the pairing prompt starts pairing itself; the user may grant or revoke this in settings meanwhile. */
    val canPairDirectly: StateFlow<Boolean> = _canPairDirectly.asStateFlow()

    fun setEnabled(enabled: Boolean) = runtime.setEnabled(enabled)

    fun refresh() {
        _canPairDirectly.value = runtime.canPairDirectly()
    }
}

/** How calls reach the computer, the switch, and the way to Bluetooth settings. */
@Composable
fun CallsScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: CallsViewModel = hiltViewModel(),
) {
    val context = LocalContext.current
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    val canPairDirectly by viewModel.canPairDirectly.collectAsStateWithLifecycle()
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { viewModel.refresh() }
    LifecycleResumeEffect(Unit) {
        viewModel.refresh()
        onPauseOrDispose {}
    }

    CallsContent(
        enabled = enabled,
        support = viewModel.support,
        canPairDirectly = canPairDirectly,
        onBack = onBack,
        onEnabled = viewModel::setEnabled,
        onOpenBluetoothSettings = { openBluetoothSettings(context) },
        // Asked only here, when the user chooses it.
        onAllowPairing = CallsRuntime.PAIR_PERMISSION?.let { { permission.launch(it) } },
        modifier = modifier,
    )
}

private fun openBluetoothSettings(context: Context) {
    try {
        context.startActivity(Intent(Settings.ACTION_BLUETOOTH_SETTINGS))
    } catch (_: ActivityNotFoundException) {
        // No such screen on this device; nothing to open.
    }
}

@Composable
internal fun CallsContent(
    enabled: Boolean,
    support: CallsSupport,
    canPairDirectly: Boolean,
    onBack: () -> Unit,
    onEnabled: (Boolean) -> Unit,
    onOpenBluetoothSettings: () -> Unit,
    /** Asks for the permission to pair from the prompt; null where there is none to ask for. */
    onAllowPairing: (() -> Unit)?,
    modifier: Modifier = Modifier,
) {
    val supported = support == CallsSupport.SUPPORTED
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.CALLS.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            item {
                L2lSection {
                    L2lListItem(
                        headlineContent = { Text(stringResource(R.string.calls_toggle_title)) },
                        supportingContent = {
                            Text(
                                when (support) {
                                    CallsSupport.SUPPORTED -> stringResource(R.string.calls_toggle_summary)
                                    CallsSupport.NO_TELEPHONY -> stringResource(R.string.calls_no_telephony)
                                    CallsSupport.NO_BLUETOOTH -> stringResource(R.string.calls_no_bluetooth)
                                },
                            )
                        },
                        trailingContent = {
                            Switch(checked = enabled && supported, onCheckedChange = onEnabled, enabled = supported)
                        },
                    )
                }
            }
            if (supported) {
                item {
                    L2lSectionHeader(stringResource(R.string.calls_how_title))
                    L2lCallout(
                        text = stringResource(R.string.calls_how_body),
                        icon = painterResource(DesignR.drawable.ic_call),
                        modifier = Modifier
                            .padding(horizontal = 16.dp)
                            .padding(bottom = 8.dp),
                    )
                }
                item {
                    L2lSection {
                        L2lListItem(
                            headlineContent = { Text(stringResource(R.string.calls_bluetooth_settings_title)) },
                            supportingContent = { Text(stringResource(R.string.calls_bluetooth_settings_summary)) },
                            trailingContent = {
                                L2lTextButton(onClick = onOpenBluetoothSettings) {
                                    Text(stringResource(R.string.calls_bluetooth_settings_action))
                                }
                            },
                        )
                        if (enabled && onAllowPairing != null) {
                            L2lDivider()
                            L2lListItem(
                                headlineContent = { Text(stringResource(R.string.calls_direct_title)) },
                                supportingContent = { Text(stringResource(R.string.calls_direct_summary)) },
                                trailingContent = {
                                    if (canPairDirectly) {
                                        StatusBadge(
                                            text = stringResource(R.string.calls_direct_allowed),
                                            tone = Tone.Success,
                                        )
                                    } else {
                                        L2lTextButton(onClick = onAllowPairing) {
                                            Text(stringResource(R.string.calls_direct_allow))
                                        }
                                    }
                                },
                            )
                        }
                    }
                }
            }
            item { Spacer(Modifier.height(16.dp)) }
        }
    }
}
