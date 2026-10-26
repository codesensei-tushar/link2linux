package dev.link2linux.android.feature.files

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.annotation.StringRes
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.core.content.IntentCompat
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dagger.hilt.android.AndroidEntryPoint
import dev.link2linux.android.core.designsystem.theme.L2lTheme

/**
 * "Send to computer" in the share sheet, for shares that carry files
 * (`EXTRA_STREAM`), one or several, of any type. Text without a file is the
 * clipboard's share target ("Copy to computer"); a text share that lands here
 * gets a hint. The activity has no window of its own: it asks which desktop
 * if there are several, hands the files to the transfer, and closes.
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
    val started by viewModel.shareStarted.collectAsStateWithLifecycle()
    val result by viewModel.shareResult.collectAsStateWithLifecycle()

    // The share's read grant lasts as long as this activity: it closes once the files are handed over.
    LaunchedEffect(result) {
        val done = result ?: return@LaunchedEffect
        if (done.unreadable < done.total) {
            val message = if (done.desktop.connected) R.string.files_sending_to else R.string.files_sending_later
            Toast.makeText(context, resources.getString(message, done.desktop.peer.name), Toast.LENGTH_SHORT).show()
        }
        toastUnreadable(context, done.unreadable)
        onDone()
    }

    val choices = desktops
    when {
        started || choices == null -> Unit
        uris.isEmpty() -> Notice(R.string.files_share_nothing, onDone)
        choices.isEmpty() -> Notice(R.string.files_share_pair_first, onDone)
        // The share target is listed only while files are on; this is a share that raced turning them off.
        !enabled -> Notice(R.string.files_share_off, onDone)
        choices.size == 1 -> LaunchedEffect(Unit) { viewModel.share(choices.single(), uris) }
        else -> DesktopChooser(
            desktops = choices,
            onChoose = { peer -> viewModel.share(choices.first { it.peer == peer }, uris) },
            onDismiss = onDone,
        )
    }
}

@Composable
private fun Notice(@StringRes message: Int, onDone: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDone,
        text = { Text(stringResource(message)) },
        confirmButton = { TextButton(onClick = onDone) { Text(stringResource(R.string.files_ok)) } },
    )
}
