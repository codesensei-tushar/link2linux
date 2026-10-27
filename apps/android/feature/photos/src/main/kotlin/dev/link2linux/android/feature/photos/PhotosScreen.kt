package dev.link2linux.android.feature.photos

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
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
import dev.link2linux.android.core.designsystem.R as DesignR
import dev.link2linux.android.core.designsystem.component.L2lCallout
import dev.link2linux.android.core.designsystem.component.L2lSectionHeader
import dev.link2linux.android.core.designsystem.component.L2lDivider
import dev.link2linux.android.core.designsystem.component.L2lListItem
import dev.link2linux.android.core.designsystem.component.L2lSection
import dev.link2linux.android.core.designsystem.component.L2lTextButton
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.StatusBadge
import dev.link2linux.android.core.designsystem.component.Tone
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature
import kotlinx.coroutines.flow.StateFlow
import javax.inject.Inject

/** The photos feature for its screen. */
@HiltViewModel
class PhotosViewModel @Inject internal constructor(private val runtime: PhotosRuntime) : ViewModel() {
    val enabled: StateFlow<Boolean> = runtime.enabled
    val access: StateFlow<PhotoAccess> = runtime.access

    fun setEnabled(enabled: Boolean) = runtime.setEnabled(enabled)

    fun refreshAccess() = runtime.refreshAccess()
}

/** Whether the computer shows the phone's recent photos, the permission that needs, and what it shares. */
@Composable
fun PhotosScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: PhotosViewModel = hiltViewModel(),
) {
    val context = LocalContext.current
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    val access by viewModel.access.collectAsStateWithLifecycle()
    var denied by rememberSaveable { mutableStateOf(false) }

    // The user comes back from the system settings page: look again.
    LifecycleResumeEffect(Unit) {
        viewModel.refreshAccess()
        onPauseOrDispose {}
    }

    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { results ->
        denied = results.values.none { it }
        // Asking again lets the user pick other photos, which the permission alone does not show.
        viewModel.refreshAccess()
    }

    PhotosContent(
        enabled = enabled,
        access = access,
        denied = denied,
        onBack = onBack,
        onEnable = { on ->
            viewModel.setEnabled(on)
            if (on && access == PhotoAccess.NONE) ask.launch(PhotoPermissions.requested())
        },
        onAsk = { ask.launch(PhotoPermissions.requested()) },
        onOpenAppSettings = { openAppSettings(context) },
        modifier = modifier,
    )
}

@Composable
internal fun PhotosContent(
    enabled: Boolean,
    access: PhotoAccess,
    denied: Boolean,
    onBack: () -> Unit,
    onEnable: (Boolean) -> Unit,
    onAsk: () -> Unit,
    onOpenAppSettings: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var showRationale by rememberSaveable { mutableStateOf(false) }

    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.PHOTOS.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            item {
                L2lSection {
                    L2lListItem(
                        headlineContent = { Text(stringResource(R.string.photos_toggle_title)) },
                        supportingContent = { Text(stringResource(R.string.photos_toggle_summary)) },
                        trailingContent = {
                            Switch(
                                checked = enabled,
                                onCheckedChange = { on ->
                                    // Explain before Android asks for the permission.
                                    if (on && access == PhotoAccess.NONE) showRationale = true else onEnable(on)
                                },
                            )
                        },
                    )
                    if (enabled) {
                        L2lDivider()
                        L2lListItem(
                            headlineContent = { Text(stringResource(R.string.photos_access_title)) },
                            supportingContent = {
                                when (access) {
                                    PhotoAccess.ALL -> StatusBadge(
                                        text = stringResource(R.string.photos_access_all),
                                        tone = Tone.Success,
                                    )
                                    PhotoAccess.SELECTED -> Text(stringResource(R.string.photos_access_selected))
                                    PhotoAccess.NONE -> Text(stringResource(R.string.photos_access_missing))
                                }
                            },
                            trailingContent = {
                                when (access) {
                                    PhotoAccess.SELECTED -> L2lTextButton(onClick = onAsk) {
                                        Text(stringResource(R.string.photos_access_pick))
                                    }
                                    // Once refused, Android may not ask again; its settings page always can.
                                    PhotoAccess.NONE -> L2lTextButton(onClick = if (denied) onOpenAppSettings else onAsk) {
                                        Text(stringResource(if (denied) R.string.photos_open_settings else R.string.photos_access_allow))
                                    }
                                    PhotoAccess.ALL -> Unit
                                }
                            },
                        )
                    }
                }
            }
            item { L2lSectionHeader(text = stringResource(R.string.photos_shared_title)) }
            item {
                L2lCallout(
                    text = stringResource(R.string.photos_shared_body),
                    tone = Tone.Accent,
                    icon = painterResource(DesignR.drawable.ic_photo_library),
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
            item { Spacer(Modifier.height(16.dp)) }
        }
    }

    if (showRationale) {
        AlertDialog(
            onDismissRequest = { showRationale = false },
            title = { Text(stringResource(R.string.photos_rationale_title)) },
            text = { Text(stringResource(R.string.photos_rationale_body)) },
            confirmButton = {
                L2lTextButton(onClick = {
                    showRationale = false
                    onEnable(true)
                }) { Text(stringResource(R.string.photos_continue)) }
            },
            dismissButton = {
                L2lTextButton(onClick = { showRationale = false }) { Text(stringResource(R.string.photos_not_now)) }
            },
        )
    }
}

private fun openAppSettings(context: Context) {
    context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)))
}
