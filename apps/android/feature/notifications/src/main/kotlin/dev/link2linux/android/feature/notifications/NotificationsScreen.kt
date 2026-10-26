package dev.link2linux.android.feature.notifications

import android.content.ActivityNotFoundException
import android.content.Context
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature

/** Settings of the notifications feature. */
@Composable
fun NotificationsScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val runtime = remember { NotificationsRuntime.get(context) }
    val settings by runtime.settings.state.collectAsStateWithLifecycle()
    val granted by runtime.accessGranted.collectAsStateWithLifecycle()

    // The user comes back from the system settings page: look again.
    LifecycleResumeEffect(Unit) {
        runtime.refreshAccess()
        onPauseOrDispose {}
    }

    NotificationsContent(
        settings = settings,
        accessGranted = granted,
        onBack = onBack,
        onEnable = {
            runtime.settings.setEnabled(true)
            openAccessSettings(context)
        },
        onEnableWithAccess = { runtime.settings.setEnabled(true) },
        onDisable = { runtime.settings.setEnabled(false) },
        onOpenAccessSettings = { openAccessSettings(context) },
        onDismissSync = runtime.settings::setDismissSync,
        onShared = runtime.settings::setShared,
        onContentHidden = runtime.settings::setContentHidden,
        modifier = modifier,
    )
}

private fun openAccessSettings(context: Context) {
    try {
        context.startActivity(NotificationAccess.settingsIntent(context))
    } catch (_: ActivityNotFoundException) {
        context.startActivity(NotificationAccess.listIntent())
    }
}

@Composable
internal fun NotificationsContent(
    settings: NotificationSettingsState,
    accessGranted: Boolean,
    onBack: () -> Unit,
    onEnable: () -> Unit,
    onEnableWithAccess: () -> Unit,
    onDisable: () -> Unit,
    onOpenAccessSettings: () -> Unit,
    onDismissSync: (Boolean) -> Unit,
    onShared: (String, Boolean) -> Unit,
    onContentHidden: (String, Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    var showRationale by rememberSaveable { mutableStateOf(false) }

    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.NOTIFICATIONS.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.notifications_toggle_title)) },
                    supportingContent = { Text(stringResource(R.string.notifications_toggle_summary)) },
                    trailingContent = {
                        Switch(
                            checked = settings.enabled,
                            onCheckedChange = { on ->
                                when {
                                    !on -> onDisable()
                                    accessGranted -> onEnableWithAccess()
                                    // Explain before sending the user to the system page.
                                    else -> showRationale = true
                                }
                            },
                        )
                    },
                )
            }
            if (settings.enabled) {
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.notifications_access_title)) },
                        supportingContent = {
                            Text(
                                stringResource(
                                    if (accessGranted) R.string.notifications_access_granted else R.string.notifications_access_missing,
                                ),
                            )
                        },
                        trailingContent = {
                            TextButton(onClick = onOpenAccessSettings) {
                                Text(
                                    stringResource(
                                        if (accessGranted) R.string.notifications_access_manage else R.string.notifications_access_open,
                                    ),
                                )
                            }
                        },
                    )
                }
                if (!accessGranted) {
                    item {
                        Text(
                            text = stringResource(R.string.notifications_access_restricted_hint),
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                        )
                    }
                }
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.notifications_dismiss_sync_title)) },
                        supportingContent = { Text(stringResource(R.string.notifications_dismiss_sync_summary)) },
                        trailingContent = { Switch(checked = settings.dismissSync, onCheckedChange = onDismissSync) },
                    )
                }
                item {
                    HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
                    ListItem(
                        headlineContent = {
                            Text(stringResource(R.string.notifications_apps_title), style = MaterialTheme.typography.titleMedium)
                        },
                        supportingContent = { Text(stringResource(R.string.notifications_apps_summary)) },
                    )
                }
                if (settings.knownApps.isEmpty()) {
                    item {
                        Text(
                            text = stringResource(R.string.notifications_apps_empty),
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                        )
                    }
                }
                items(settings.knownApps.entries.sortedBy { it.value.lowercase() }, key = { it.key }) { (pkg, name) ->
                    val shared = pkg !in settings.excluded
                    ListItem(
                        headlineContent = { Text(name) },
                        supportingContent = {
                            if (shared) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Checkbox(
                                        checked = pkg in settings.contentHidden,
                                        onCheckedChange = { onContentHidden(pkg, it) },
                                    )
                                    Text(stringResource(R.string.notifications_app_hide_text))
                                }
                            } else {
                                Text(pkg)
                            }
                        },
                        trailingContent = { Switch(checked = shared, onCheckedChange = { onShared(pkg, it) }) },
                    )
                }
            }
        }
    }

    if (showRationale) {
        AlertDialog(
            onDismissRequest = { showRationale = false },
            title = { Text(stringResource(R.string.notifications_rationale_title)) },
            text = { Text(stringResource(R.string.notifications_rationale_body)) },
            confirmButton = {
                TextButton(onClick = {
                    showRationale = false
                    onEnable()
                }) { Text(stringResource(R.string.notifications_rationale_continue)) }
            },
            dismissButton = {
                TextButton(onClick = { showRationale = false }) {
                    Text(stringResource(R.string.notifications_rationale_cancel))
                }
            },
        )
    }
}
