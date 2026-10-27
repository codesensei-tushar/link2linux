package dev.link2linux.android.feature.ring

import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.designsystem.component.L2lButton
import dev.link2linux.android.core.designsystem.component.L2lCallout
import dev.link2linux.android.core.designsystem.component.L2lCard
import dev.link2linux.android.core.designsystem.component.L2lListItem
import dev.link2linux.android.core.designsystem.component.L2lSection
import dev.link2linux.android.core.designsystem.component.L2lSectionHeader
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.StatusBadge
import dev.link2linux.android.core.designsystem.component.Tone
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.designsystem.R as DesignR
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
                    L2lCard(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 16.dp, vertical = 4.dp),
                    ) {
                        L2lListItem(
                            headlineContent = { Text(stringResource(R.string.ring_ringing_title, ringingFor)) },
                            supportingContent = {
                                StatusBadge(text = stringResource(R.string.ring_ringing_badge), tone = Tone.Accent)
                            },
                            trailingContent = {
                                L2lButton(onClick = onStop) { Text(stringResource(R.string.ring_stop_ringing)) }
                            },
                        )
                    }
                }
            }
            item {
                L2lSection {
                    L2lListItem(
                        headlineContent = { Text(stringResource(R.string.ring_toggle_title)) },
                        supportingContent = { Text(stringResource(R.string.ring_toggle_summary)) },
                        trailingContent = { Switch(checked = enabled, onCheckedChange = onEnabled) },
                    )
                }
            }
            item {
                L2lSectionHeader(stringResource(R.string.ring_how_title))
                L2lCallout(
                    text = stringResource(R.string.ring_how_body),
                    icon = painterResource(DesignR.drawable.ic_ring),
                    modifier = Modifier
                        .padding(horizontal = 16.dp)
                        .padding(bottom = 8.dp),
                )
            }
            item { Spacer(Modifier.height(16.dp)) }
        }
    }
}
