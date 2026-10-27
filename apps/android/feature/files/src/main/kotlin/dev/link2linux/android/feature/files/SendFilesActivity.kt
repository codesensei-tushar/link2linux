package dev.link2linux.android.feature.files

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.text.format.Formatter
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import dev.link2linux.android.core.designsystem.component.L2lTextButton
import dev.link2linux.android.core.designsystem.component.StatusBadge
import dev.link2linux.android.core.designsystem.component.Tone
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.content.IntentCompat
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.AndroidEntryPoint
import dev.link2linux.android.core.designsystem.theme.L2lTheme
import kotlinx.coroutines.delay

/**
 * "Send to computer" in the share sheet, for shares that carry files
 * (`EXTRA_STREAM`), one or several, of any type. Text without a file is the
 * clipboard's share target ("Copy to computer"); a text share that lands here
 * gets a hint. The activity has no window of its own, only dialogs: one
 * while the documents are read, then one that asks the user to confirm
 * which files go to which desktop. It hands them to the transfer and
 * closes.
 */
@AndroidEntryPoint
class SendFilesActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uris = sharedUris(intent)
        setContent {
            L2lTheme {
                SendFiles(uris = uris, onDone = ::finish)
            }
        }
    }

    /**
     * The files of the share. The extras come from another app, and a Bundle
     * holding a class this app cannot load throws when it is read; such a
     * share has nothing to send, rather than ending the process and every
     * session with it.
     */
    private fun sharedUris(intent: Intent): List<Uri> = try {
        when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty().filterNotNull()
            else -> emptyList()
        }
    } catch (_: RuntimeException) {
        emptyList()
    }
}

@Composable
private fun SendFiles(uris: List<Uri>, onDone: () -> Unit, viewModel: FilesViewModel = hiltViewModel()) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val enabled by viewModel.enabled.collectAsStateWithLifecycle()
    val desktops by viewModel.desktops.collectAsStateWithLifecycle()
    val share by viewModel.share.collectAsStateWithLifecycle()

    LaunchedEffect(share) {
        val done = share as? ShareState.Done ?: return@LaunchedEffect
        done.desktop?.let { desktop ->
            val message = if (desktop.connected) R.string.files_sending_to else R.string.files_sending_later
            Toast.makeText(context, resources.getString(message, desktop.peer.name), Toast.LENGTH_SHORT).show()
        }
        onDone()
    }

    val choices = desktops
    when (val state = share) {
        is ShareState.Done -> Unit
        ShareState.Idle -> when {
            uris.isEmpty() -> Notice(R.string.files_share_nothing, onDone)
            // Shown while the paired desktops load, so the window never sits over the screen unseen.
            choices == null -> Preparing(uris.size, onCancel = onDone)
            choices.isEmpty() -> Notice(R.string.files_share_pair_first, onDone)
            // The share target is listed only while files are on; this is a share that raced turning them off.
            !enabled -> Notice(R.string.files_share_off, onDone)
            else -> LaunchedEffect(Unit) { viewModel.prepareShare(uris) }
        }
        ShareState.Preparing -> Preparing(uris.size, onCancel = viewModel::cancelShare)
        is ShareState.Ready -> when {
            choices.isNullOrEmpty() -> Notice(R.string.files_share_pair_first, onDone = viewModel::cancelShare)
            state.prepared.count == 0 -> Notice(R.string.files_share_none_readable, onDone = viewModel::cancelShare)
            else -> ConfirmShare(state.prepared, choices, onSend = viewModel::confirmShare, onCancel = viewModel::cancelShare)
        }
    }
}

/** Shown while the shared documents are read, which a slow provider or a copy can make take a while. */
@Composable
private fun Preparing(count: Int, onCancel: () -> Unit) {
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text(pluralStringResource(R.plurals.files_share_preparing, count, count)) },
        text = { LinearProgressIndicator(modifier = Modifier.fillMaxWidth()) },
        confirmButton = {},
        dismissButton = { L2lTextButton(onClick = onCancel) { Text(stringResource(R.string.files_cancel)) } },
    )
}

/**
 * Asks the user to confirm what goes where: another app can start the
 * share target without them, so nothing goes before this, even to the
 * only paired desktop.
 */
@Composable
private fun ConfirmShare(
    prepared: FilesRuntime.Prepared,
    desktops: List<DesktopChoice>,
    onSend: (DesktopChoice) -> Unit,
    onCancel: () -> Unit,
) {
    val context = LocalContext.current
    // The list puts connected desktops first.
    var chosen by rememberSaveable { mutableStateOf(desktops.first().peer.id) }
    // The chosen desktop may be unpaired while this shows: the first one is chosen then.
    val selected = desktops.firstOrNull { it.peer.id == chosen } ?: desktops.first()
    // Another app can start this at the moment the user taps there; a tap right as it appears is not an answer.
    var armed by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        delay(CONFIRM_ARM_MILLIS)
        armed = true
    }
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text(pluralStringResource(R.plurals.files_share_confirm, prepared.count, prepared.count)) },
        text = {
            Column {
                prepared.names.take(SHOWN_NAMES).forEach { Text(shortened(it), style = MaterialTheme.typography.bodyMedium) }
                val more = prepared.count - SHOWN_NAMES
                if (more > 0) Text(pluralStringResource(R.plurals.files_share_more, more, more), style = MaterialTheme.typography.bodyMedium)
                Text(
                    stringResource(R.string.files_share_total, Formatter.formatShortFileSize(context, prepared.totalBytes)),
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(top = 8.dp),
                )
                if (prepared.unreadable > 0) {
                    Text(
                        pluralStringResource(R.plurals.files_share_left_out, prepared.unreadable, prepared.unreadable),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (desktops.size == 1) {
                    Text(
                        stringResource(R.string.files_share_to, desktops.single().peer.name),
                        style = MaterialTheme.typography.titleSmall,
                        modifier = Modifier.padding(top = 16.dp),
                    )
                } else {
                    Text(
                        stringResource(R.string.files_choose_desktop),
                        style = MaterialTheme.typography.titleSmall,
                        modifier = Modifier.padding(top = 16.dp),
                    )
                    desktops.forEach { choice ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .selectable(selected = choice.peer.id == selected.peer.id, onClick = { chosen = choice.peer.id }),
                        ) {
                            RadioButton(selected = choice.peer.id == selected.peer.id, onClick = null)
                            Column(modifier = Modifier.padding(start = 8.dp)) {
                                Text(choice.peer.name)
                                if (choice.connected) {
                                    StatusBadge(
                                        text = stringResource(R.string.files_desktop_connected),
                                        tone = Tone.Success,
                                        modifier = Modifier.padding(top = 4.dp),
                                    )
                                } else {
                                    Text(
                                        stringResource(R.string.files_desktop_away),
                                        style = MaterialTheme.typography.bodySmall,
                                        modifier = Modifier.padding(top = 4.dp),
                                    )
                                }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            L2lTextButton(onClick = { onSend(selected) }, enabled = armed) { Text(stringResource(R.string.files_send_now)) }
        },
        dismissButton = { L2lTextButton(onClick = onCancel) { Text(stringResource(R.string.files_cancel)) } },
    )
}

/** A name cut for a dialog line, without splitting a character. */
private fun shortened(name: String): String {
    if (name.length <= SHOWN_NAME_CHARS) return name
    val cut = name.take(SHOWN_NAME_CHARS - 1).let { if (it.last().isHighSurrogate()) it.dropLast(1) else it }
    return "$cut…"
}

private const val SHOWN_NAMES = 3
private const val SHOWN_NAME_CHARS = 40

/** How long after the confirmation shows its Send button ignores taps. */
private const val CONFIRM_ARM_MILLIS = 500L

@Composable
private fun Notice(@StringRes message: Int, onDone: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDone,
        text = { Text(stringResource(message)) },
        confirmButton = { L2lTextButton(onClick = onDone) { Text(stringResource(R.string.files_ok)) } },
    )
}
