package dev.link2linux.android.feature.files

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.files.Direction
import dev.link2linux.android.core.files.FilePeer
import dev.link2linux.android.core.files.Transfer
import dev.link2linux.android.core.files.TransferKey
import dev.link2linux.android.core.model.Feature
import kotlinx.coroutines.launch

/** Files sent between the phone and the computer: the setting, a way to send, and recent transfers. */
@Composable
fun FilesScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    viewModel: FilesViewModel = hiltViewModel(),
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    val storageGranted by viewModel.storageGranted.collectAsStateWithLifecycle()
    val transfers by viewModel.transfers.collectAsStateWithLifecycle()
    val desktops by viewModel.desktops.collectAsStateWithLifecycle()
    var picked by remember { mutableStateOf<List<Uri>?>(null) }
    var storageDenied by rememberSaveable { mutableStateOf(false) }

    // The user comes back from the system settings page: look again.
    LifecycleResumeEffect(Unit) {
        viewModel.refreshStorageAccess()
        onPauseOrDispose {}
    }

    val askStorage = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        storageDenied = !granted
        viewModel.refreshStorageAccess()
    }
    val send: (FilePeer, List<Uri>) -> Unit = { peer, uris ->
        scope.launch { toastUnreadable(context, viewModel.send(peer, uris).await()) }
    }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isEmpty()) return@rememberLauncherForActivityResult
        // Their read grants are kept once a desktop is chosen; a dismissed choice keeps nothing.
        val choices = desktops.orEmpty()
        if (choices.size == 1) send(choices.single().peer, uris) else picked = uris
    }

    FilesContent(
        enabled = enabled,
        storageGranted = storageGranted,
        storageDenied = storageDenied,
        hasDesktops = !desktops.isNullOrEmpty(),
        transfers = transfers,
        onBack = onBack,
        onEnable = { on ->
            viewModel.setEnabled(on)
            if (on && !storageGranted) askStorage.launch(StorageAccess.PERMISSION)
        },
        onAllowStorage = { askStorage.launch(StorageAccess.PERMISSION) },
        onOpenAppSettings = { openAppSettings(context) },
        onSend = { pick.launch(arrayOf("*/*")) },
        onCancel = viewModel::cancel,
        onOpen = { openReceived(context, it) },
        onDismiss = viewModel::dismiss,
        modifier = modifier,
    )

    picked?.let { uris ->
        DesktopChooser(
            desktops = desktops.orEmpty(),
            onChoose = { peer ->
                picked = null
                send(peer, uris)
            },
            onDismiss = { picked = null },
        )
    }
}

@Composable
internal fun FilesContent(
    enabled: Boolean,
    storageGranted: Boolean,
    storageDenied: Boolean,
    hasDesktops: Boolean,
    transfers: List<Transfer>,
    onBack: () -> Unit,
    onEnable: (Boolean) -> Unit,
    onAllowStorage: () -> Unit,
    onOpenAppSettings: () -> Unit,
    onSend: () -> Unit,
    onCancel: (TransferKey) -> Unit,
    onOpen: (Transfer) -> Unit,
    onDismiss: (TransferKey) -> Unit,
    modifier: Modifier = Modifier,
) {
    var showRationale by rememberSaveable { mutableStateOf(false) }

    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.FILES.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.files_toggle_title)) },
                    supportingContent = { Text(stringResource(R.string.files_toggle_summary)) },
                    trailingContent = {
                        Switch(
                            checked = enabled,
                            onCheckedChange = { on ->
                                // Explain before Android asks for storage access.
                                if (on && !storageGranted) showRationale = true else onEnable(on)
                            },
                        )
                    },
                )
            }
            if (enabled && StorageAccess.needsPermission) {
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.files_storage_title)) },
                        supportingContent = {
                            Text(stringResource(if (storageGranted) R.string.files_storage_granted else R.string.files_storage_missing))
                        },
                        trailingContent = {
                            if (!storageGranted) {
                                // Once refused, Android may not ask again; its settings page always can.
                                TextButton(onClick = if (storageDenied) onOpenAppSettings else onAllowStorage) {
                                    Text(stringResource(if (storageDenied) R.string.files_open_settings else R.string.files_storage_allow))
                                }
                            }
                        },
                    )
                }
            }
            if (enabled) {
                item {
                    Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                        Button(onClick = onSend, enabled = hasDesktops, modifier = Modifier.fillMaxWidth()) {
                            Text(stringResource(R.string.files_send))
                        }
                        Text(
                            text = stringResource(if (hasDesktops) R.string.files_saved_in_downloads else R.string.files_pair_first),
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                    }
                }
            }
            item {
                HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
                ListItem(
                    headlineContent = {
                        Text(stringResource(R.string.files_recent_title), style = MaterialTheme.typography.titleMedium)
                    },
                )
            }
            if (transfers.isEmpty()) {
                item {
                    Text(
                        text = stringResource(R.string.files_recent_empty),
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    )
                }
            }
            items(transfers, key = { it.key.toString() }) { transfer ->
                TransferRow(transfer, onCancel = onCancel, onOpen = onOpen, onDismiss = onDismiss)
            }
        }
    }

    if (showRationale) {
        AlertDialog(
            onDismissRequest = { showRationale = false },
            title = { Text(stringResource(R.string.files_storage_rationale_title)) },
            text = { Text(stringResource(R.string.files_storage_rationale_body)) },
            confirmButton = {
                TextButton(onClick = {
                    showRationale = false
                    onEnable(true)
                }) { Text(stringResource(R.string.files_continue)) }
            },
            dismissButton = {
                TextButton(onClick = { showRationale = false }) { Text(stringResource(R.string.files_not_now)) }
            },
        )
    }
}

@Composable
private fun TransferRow(
    transfer: Transfer,
    onCancel: (TransferKey) -> Unit,
    onOpen: (Transfer) -> Unit,
    onDismiss: (TransferKey) -> Unit,
) {
    val incoming = transfer.key.direction == Direction.INCOMING
    ListItem(
        headlineContent = { Text(TransferText.names(transfer).resolve()) },
        supportingContent = {
            Column {
                Text(TransferText.status(transfer).resolve())
                TransferText.reason(transfer)?.let { Text(it.resolve(), style = MaterialTheme.typography.bodySmall) }
                if (!transfer.state.isFinished) {
                    LinearProgressIndicator(
                        progress = { TransferText.permille(transfer) / 1000f },
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 4.dp),
                    )
                }
            }
        },
        leadingContent = {
            Icon(painterResource(if (incoming) R.drawable.ic_file_download else R.drawable.ic_file_upload), contentDescription = null)
        },
        trailingContent = {
            when {
                !transfer.state.isFinished -> TextButton(onClick = { onCancel(transfer.key) }) {
                    Text(stringResource(R.string.files_cancel))
                }
                incoming && transfer.saved.isNotEmpty() -> TextButton(onClick = { onOpen(transfer) }) {
                    Text(stringResource(R.string.files_open))
                }
                else -> TextButton(onClick = { onDismiss(transfer.key) }) { Text(stringResource(R.string.files_remove)) }
            }
        },
    )
}

/** Asks which paired desktop gets the files. */
@Composable
internal fun DesktopChooser(
    desktops: List<DesktopChoice>,
    onChoose: (FilePeer) -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.files_choose_desktop)) },
        text = {
            Column {
                desktops.forEach { choice ->
                    ListItem(
                        headlineContent = { Text(choice.peer.name) },
                        supportingContent = {
                            Text(stringResource(if (choice.connected) R.string.files_desktop_connected else R.string.files_desktop_away))
                        },
                        modifier = Modifier.clickable { onChoose(choice.peer) },
                    )
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.files_cancel)) } },
    )
}

/** Opens a received file with the app the user picks, or the Downloads list for several. */
private fun openReceived(context: Context, transfer: Transfer) {
    val single = transfer.saved.singleOrNull()?.takeIf { it.uri.isNotEmpty() }
    val intent = if (single != null) {
        Intent(Intent.ACTION_VIEW)
            .setDataAndType(single.uri.toUri(), single.mimeType)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    } else {
        Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)
    }
    try {
        context.startActivity(intent)
    } catch (_: ActivityNotFoundException) {
        Toast.makeText(context, R.string.files_no_app, Toast.LENGTH_SHORT).show()
    }
}

private fun openAppSettings(context: Context) {
    context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)))
}

internal fun toastUnreadable(context: Context, unreadable: Int) {
    if (unreadable > 0) {
        Toast.makeText(context, context.resources.getQuantityString(R.plurals.files_unreadable, unreadable, unreadable), Toast.LENGTH_LONG).show()
    }
}
