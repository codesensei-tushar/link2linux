package dev.link2linux.android.feature.status

import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
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

/** The phone status feature for its screen. */
@HiltViewModel
class StatusViewModel @Inject internal constructor(private val runtime: StatusRuntime) : ViewModel() {
    val enabled: StateFlow<Boolean> = runtime.enabled

    fun setEnabled(enabled: Boolean) = runtime.setEnabled(enabled)
}

/** Whether the computer shows the phone's battery, signal and sound mode, and what that shares. */
@Composable
fun StatusScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: StatusViewModel = hiltViewModel(),
) {
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    StatusContent(enabled = enabled, onBack = onBack, onEnabled = viewModel::setEnabled, modifier = modifier)
}

@Composable
internal fun StatusContent(
    enabled: Boolean,
    onBack: () -> Unit,
    onEnabled: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.STATUS.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.status_toggle_title)) },
                    supportingContent = { Text(stringResource(R.string.status_toggle_summary)) },
                    trailingContent = { Switch(checked = enabled, onCheckedChange = onEnabled) },
                )
            }
            item {
                HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
                ListItem(
                    headlineContent = {
                        Text(stringResource(R.string.status_shared_title), style = MaterialTheme.typography.titleMedium)
                    },
                    supportingContent = { Text(stringResource(R.string.status_shared_body)) },
                )
            }
        }
    }
}
