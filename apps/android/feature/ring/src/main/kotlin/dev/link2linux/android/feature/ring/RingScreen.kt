package dev.link2linux.android.feature.ring

import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature
import kotlinx.coroutines.flow.StateFlow
import javax.inject.Inject

/** The ring feature for its screen. */
@HiltViewModel
class RingViewModel @Inject internal constructor(private val runtime: RingRuntime) : ViewModel() {
    val enabled: StateFlow<Boolean> = runtime.enabled

    /** The computer the phone rings for, or null while it does not ring. */
    val ringingFor: StateFlow<String?> = runtime.engine.ringingFor

    fun setEnabled(enabled: Boolean) = runtime.setEnabled(enabled)

    fun stop() = runtime.stop()
}

/** Whether a computer may ring the phone, and a way to stop it, also without the notification. */
@Composable
fun RingScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: RingViewModel = hiltViewModel(),
) {
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    val ringingFor by viewModel.ringingFor.collectAsStateWithLifecycle()
    RingContent(
        enabled = enabled,
        ringingFor = ringingFor,
        onBack = onBack,
        onEnabled = viewModel::setEnabled,
        onStop = viewModel::stop,
        modifier = modifier,
    )
}

@Composable
internal fun RingContent(
    enabled: Boolean,
    ringingFor: String?,
    onBack: () -> Unit,
    onEnabled: (Boolean) -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.RING.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            if (ringingFor != null) {
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.ring_ringing_title, ringingFor)) },
                        trailingContent = { Button(onClick = onStop) { Text(stringResource(R.string.ring_stop_ringing)) } },
                    )
                }
            }
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.ring_toggle_title)) },
                    supportingContent = { Text(stringResource(R.string.ring_toggle_summary)) },
                    trailingContent = { Switch(checked = enabled, onCheckedChange = onEnabled) },
                )
            }
            item {
                HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
                ListItem(
                    headlineContent = {
                        Text(stringResource(R.string.ring_how_title), style = MaterialTheme.typography.titleMedium)
                    },
                    supportingContent = { Text(stringResource(R.string.ring_how_body)) },
                )
            }
        }
    }
}
