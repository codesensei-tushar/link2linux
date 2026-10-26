package dev.link2linux.android.feature.pairing

import android.app.Activity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.hilt.lifecycle.viewmodel.compose.hiltViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle

/**
 * After a pairing with a computer that advertises over Bluetooth: offers to
 * let Android notice when the computer is near. The user can skip it, and
 * the permission and the system's chooser appear only after tapping the
 * button. The offer's state lives in [CompanionOfferViewModel], one per
 * pairing, so Android's answer reaches the screen that is there when it
 * comes, after a rotation too. [pairedAt] tells a new pairing with the same
 * computer from the last.
 */
@Composable
internal fun CompanionOffer(bluetoothAddress: String, pairedAt: Long, modifier: Modifier = Modifier) {
    val model: CompanionOfferViewModel = hiltViewModel(key = "companion:$bluetoothAddress:$pairedAt")
    if (!model.isAvailable) return
    val step by model.step.collectAsStateWithLifecycle()
    if (step == OfferStep.SKIPPED) return

    val chooser = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        model.onChooserResult(result.resultCode == Activity.RESULT_OK, bluetoothAddress)
    }
    // Collected only while the screen is started, so the chooser opens in front of the user.
    val pending by model.chooser.collectAsStateWithLifecycle()
    LaunchedEffect(pending) {
        val sender = pending ?: return@LaunchedEffect
        model.onChooserShown()
        chooser.launch(IntentSenderRequest.Builder(sender).build())
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        model.onPermission(granted, bluetoothAddress)
    }

    Column(
        modifier = modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(
            text = stringResource(R.string.feature_pairing_companion_title),
            style = MaterialTheme.typography.titleMedium,
            textAlign = TextAlign.Center,
        )
        val body = when (step) {
            OfferStep.DONE -> R.string.feature_pairing_companion_done
            OfferStep.NOT_FOUND -> R.string.feature_pairing_companion_not_found
            OfferStep.UNAVAILABLE -> R.string.feature_pairing_companion_unavailable
            OfferStep.WORKING -> R.string.feature_pairing_companion_working
            else -> R.string.feature_pairing_companion_body
        }
        Text(
            text = stringResource(body),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        if (step == OfferStep.ASK || step == OfferStep.NOT_FOUND) {
            Button(
                onClick = { model.permission?.let(permission::launch) ?: model.begin(bluetoothAddress) },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.feature_pairing_companion_action)) }
        }
        if (step != OfferStep.DONE && step != OfferStep.WORKING) {
            OutlinedButton(onClick = model::skip, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_companion_skip))
            }
        }
    }
}
