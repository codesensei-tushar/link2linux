package dev.link2linux.android.feature.clipboard

import android.app.StatusBarManager
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.graphics.drawable.Icon
import android.os.Build
import androidx.annotation.RequiresApi
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.link2linux.android.core.clipboard.ConnectedDesktop
import dev.link2linux.android.core.designsystem.component.L2lTopAppBar
import dev.link2linux.android.core.designsystem.component.titleRes
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.designsystem.R as DesignR

/** Settings of the clipboard feature, and a way to send the clipboard. */
@Composable
fun ClipboardScreen(
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val runtime = remember { ClipboardRuntime.get(context) }
    val enabled by runtime.enabled.collectAsStateWithLifecycle()
    val desktops by runtime.engine.connected.collectAsStateWithLifecycle()

    ClipboardContent(
        enabled = enabled,
        desktops = desktops,
        onBack = onBack,
        onEnabled = runtime::setEnabled,
        onSend = { sendClipboard(context, runtime) },
        onAddTile = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            { requestTile(context) }
        } else {
            null
        },
        modifier = modifier,
    )
}

/**
 * "Send clipboard" for the app's home screen, while the feature is on and
 * a computer is connected.
 */
@Composable
fun SendClipboardButton(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val runtime = remember { ClipboardRuntime.get(context) }
    val enabled by runtime.enabled.collectAsStateWithLifecycle()
    val desktops by runtime.engine.connected.collectAsStateWithLifecycle()
    if (!enabled || desktops.isEmpty()) return
    OutlinedButton(
        onClick = { sendClipboard(context, runtime) },
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Text(stringResource(R.string.clipboard_send_action))
    }
}

/** The app has focus while the user taps its button, so it reads the clipboard itself. */
private fun sendClipboard(context: Context, runtime: ClipboardRuntime) {
    val refusal = runtime.refusalBeforeReading()
    if (refusal != null) {
        runtime.show(refusal)
    } else {
        runtime.shareClipboard(context.getSystemService(ClipboardManager::class.java)?.primaryClip)
    }
}

/** Asks Android to offer the tile, which the user then confirms. */
@RequiresApi(Build.VERSION_CODES.TIRAMISU)
private fun requestTile(context: Context) {
    val manager = context.getSystemService(StatusBarManager::class.java) ?: return
    manager.requestAddTileService(
        ComponentName(context, ClipboardTileService::class.java),
        context.getString(R.string.clipboard_tile_label),
        Icon.createWithResource(context, DesignR.drawable.ic_content_paste),
        context.mainExecutor,
    ) { }
}

@Composable
internal fun ClipboardContent(
    enabled: Boolean,
    desktops: List<ConnectedDesktop>,
    onBack: () -> Unit,
    onEnabled: (Boolean) -> Unit,
    onSend: () -> Unit,
    onAddTile: (() -> Unit)?,
    modifier: Modifier = Modifier,
) {
    Scaffold(
        modifier = modifier,
        topBar = { L2lTopAppBar(title = stringResource(Feature.CLIPBOARD.titleRes), onBack = onBack) },
    ) { padding ->
        LazyColumn(contentPadding = padding) {
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.clipboard_toggle_title)) },
                    supportingContent = { Text(stringResource(R.string.clipboard_toggle_summary)) },
                    trailingContent = { Switch(checked = enabled, onCheckedChange = onEnabled) },
                )
            }
            if (enabled) {
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.clipboard_send_title)) },
                        supportingContent = {
                            Text(
                                if (desktops.isEmpty()) {
                                    stringResource(R.string.clipboard_send_none)
                                } else {
                                    stringResource(R.string.clipboard_send_to, desktops.joinToString(", ") { it.name })
                                },
                            )
                        },
                        trailingContent = {
                            TextButton(onClick = onSend, enabled = desktops.isNotEmpty()) {
                                Text(stringResource(R.string.clipboard_send_action))
                            }
                        },
                    )
                }
                if (onAddTile != null) {
                    item {
                        ListItem(
                            headlineContent = { Text(stringResource(R.string.clipboard_add_tile_title)) },
                            supportingContent = { Text(stringResource(R.string.clipboard_add_tile_summary)) },
                            trailingContent = {
                                TextButton(onClick = onAddTile) { Text(stringResource(R.string.clipboard_add_tile_action)) }
                            },
                        )
                    }
                }
                item {
                    HorizontalDivider(modifier = Modifier.padding(vertical = 8.dp))
                    ListItem(
                        headlineContent = {
                            Text(stringResource(R.string.clipboard_how_title), style = MaterialTheme.typography.titleMedium)
                        },
                        supportingContent = { Text(stringResource(R.string.clipboard_how_body)) },
                    )
                }
            }
        }
    }
}
