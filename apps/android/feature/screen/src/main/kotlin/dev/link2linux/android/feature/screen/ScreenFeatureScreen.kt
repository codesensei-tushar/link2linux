package dev.link2linux.android.feature.screen

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
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
import androidx.core.content.ContextCompat
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.designsystem.R as DesignR
import dev.link2linux.android.core.designsystem.component.L2lButton
import dev.link2linux.android.core.designsystem.component.L2lCallout
import dev.link2linux.android.core.designsystem.component.L2lDivider
import dev.link2linux.android.core.designsystem.component.L2lListItem
import dev.link2linux.android.core.designsystem.component.L2lOutlinedButton
import dev.link2linux.android.core.designsystem.component.L2lSection
import dev.link2linux.android.core.designsystem.component.L2lTextButton
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.StatusBadge
import dev.link2linux.android.core.designsystem.component.Tone
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.screen.ShareState
import kotlinx.coroutines.flow.StateFlow
import javax.inject.Inject

/** The screen feature for its screen. */
@HiltViewModel
class ScreenViewModel @Inject internal constructor(private val runtime: ScreenRuntime) : ViewModel() {
    val enabled: StateFlow<Boolean> = runtime.enabled
    val controlAllowed: StateFlow<Boolean> = runtime.controlAllowed
    val controlServiceOn: StateFlow<Boolean> = runtime.controlServiceOn
    val share: StateFlow<ShareState> = runtime.engine.share

    fun setEnabled(enabled: Boolean) = runtime.setEnabled(enabled)

    fun setControlAllowed(allowed: Boolean) = runtime.setControlAllowed(allowed)

    fun stop() = runtime.stop()
}

/**
 * Whether a computer may show the phone's screen, whether it may also
 * control the phone, and a way to stop sharing. Control asks for Android's
 * accessibility service, after saying plainly what that service does.
 */
@Composable
fun ScreenFeatureScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: ScreenViewModel = hiltViewModel(),
) {
    val context = LocalContext.current
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    val controlAllowed by viewModel.controlAllowed.collectAsStateWithLifecycle()
    val serviceOn by viewModel.controlServiceOn.collectAsStateWithLifecycle()
    val share by viewModel.share.collectAsStateWithLifecycle()
    var notificationsDenied by rememberSaveable { mutableStateOf(false) }
    // The question comes as a notification while the app is in the background.
    val askNotifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        notificationsDenied = !granted
    }
    ScreenContent(
        enabled = enabled,
        share = share,
        controlAllowed = controlAllowed,
        serviceOn = serviceOn,
        notificationsDenied = notificationsDenied,
        onBack = onBack,
        onEnabled = { on ->
            viewModel.setEnabled(on)
            if (on && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        },
        onStop = viewModel::stop,
        onControl = { allowed ->
            viewModel.setControlAllowed(allowed)
            if (allowed && !serviceOn) context.open(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
        },
        onOpenAccessibility = { context.open(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) },
        onOpenAppInfo = {
            context.open(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)))
        },
        modifier = modifier,
    )
}

private fun Context.open(intent: Intent) {
    try {
        startActivity(intent)
    } catch (_: ActivityNotFoundException) {
        // A phone without this settings page; the text says where to look.
    }
}

@Composable
internal fun ScreenContent(
    enabled: Boolean,
    share: ShareState,
    controlAllowed: Boolean,
    serviceOn: Boolean,
    notificationsDenied: Boolean,
    onBack: () -> Unit,
    onEnabled: (Boolean) -> Unit,
    onStop: () -> Unit,
    onControl: (Boolean) -> Unit,
    onOpenAccessibility: () -> Unit,
    onOpenAppInfo: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var disclosing by rememberSaveable { mutableStateOf(false) }
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.SCREEN.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            when (share) {
                is ShareState.Sharing -> item {
                    L2lSection {
                        L2lListItem(
                            headlineContent = { Text(stringResource(R.string.screen_sharing_title, share.desktopName)) },
                            supportingContent = {
                                StatusBadge(text = stringResource(R.string.screen_status_sharing), tone = Tone.Accent)
                            },
                            trailingContent = {
                                L2lButton(onClick = onStop) { Text(stringResource(R.string.screen_stop_sharing)) }
                            },
                        )
                    }
                }
                is ShareState.Asking -> item {
                    L2lSection {
                        L2lListItem(
                            headlineContent = { Text(stringResource(R.string.screen_asking_title, share.desktopName)) },
                            supportingContent = {
                                StatusBadge(text = stringResource(R.string.screen_status_waiting), tone = Tone.Warning)
                            },
                        )
                    }
                }
                ShareState.Idle -> Unit
            }
            item {
                L2lSection {
                    L2lListItem(
                        headlineContent = { Text(stringResource(R.string.screen_toggle_title)) },
                        supportingContent = { Text(stringResource(R.string.screen_toggle_summary)) },
                        trailingContent = { Switch(checked = enabled, onCheckedChange = onEnabled) },
                    )
                }
            }
            if (enabled && notificationsDenied) {
                item {
                    L2lCallout(
                        text = stringResource(R.string.screen_notifications_needed),
                        tone = Tone.Warning,
                        icon = painterResource(DesignR.drawable.ic_notifications),
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
            }
            item {
                L2lSection {
                    L2lListItem(
                        headlineContent = { Text(stringResource(R.string.screen_control_title)) },
                        supportingContent = { Text(stringResource(R.string.screen_control_summary)) },
                        trailingContent = {
                            Switch(
                                checked = controlAllowed,
                                // Turning it on first says what the accessibility service does.
                                onCheckedChange = { on -> if (on) disclosing = true else onControl(false) },
                            )
                        },
                    )
                    if (controlAllowed && !serviceOn) {
                        L2lDivider()
                        L2lListItem(
                            headlineContent = { Text(stringResource(R.string.screen_control_service_off_title)) },
                            supportingContent = {
                                Column {
                                    Text(stringResource(R.string.screen_control_service_off_body))
                                    // Android 13 and later restrict the setting for apps installed from outside a store.
                                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                                        Text(
                                            stringResource(R.string.screen_control_restricted_body),
                                            modifier = Modifier.padding(top = 8.dp),
                                        )
                                        L2lOutlinedButton(onClick = onOpenAppInfo, modifier = Modifier.padding(top = 8.dp)) {
                                            Text(stringResource(R.string.screen_open_app_info))
                                        }
                                    }
                                }
                            },
                            trailingContent = {
                                L2lButton(onClick = onOpenAccessibility) {
                                    Text(stringResource(R.string.screen_open_accessibility_settings))
                                }
                            },
                        )
                    }
                    if (controlAllowed && serviceOn) {
                        L2lDivider()
                        L2lListItem(headlineContent = { Text(stringResource(R.string.screen_control_ready)) })
                    }
                }
            }
            item {
                L2lCallout(
                    text = stringResource(R.string.screen_how_body),
                    tone = Tone.Accent,
                    icon = painterResource(DesignR.drawable.ic_screen_share),
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
            item { Spacer(Modifier.height(16.dp)) }
        }
    }
    if (disclosing) {
        AlertDialog(
            onDismissRequest = { disclosing = false },
            title = { Text(stringResource(R.string.screen_control_disclosure_title)) },
            text = {
                Column(modifier = Modifier.verticalScroll(rememberScrollState())) {
                    Text(stringResource(R.string.screen_control_disclosure_body))
                }
            },
            confirmButton = {
                L2lTextButton(onClick = {
                    disclosing = false
                    onControl(true)
                }) { Text(stringResource(R.string.screen_control_disclosure_allow)) }
            },
            dismissButton = {
                L2lTextButton(onClick = { disclosing = false }) { Text(stringResource(R.string.screen_control_disclosure_cancel)) }
            },
        )
    }
}
