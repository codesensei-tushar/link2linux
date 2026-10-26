package dev.link2linux.android.feature.pairing

import android.app.Activity
import android.content.IntentSender
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp

private enum class OfferStep { ASK, WORKING, DONE, NOT_FOUND, UNAVAILABLE, SKIPPED }

/**
 * After a pairing with a computer that advertises over Bluetooth: offers to
 * let Android notice when the computer is near. The user can skip it, and
 * the permission and the system's chooser appear only after tapping the
 * button.
 */
@Composable
internal fun CompanionOffer(bluetoothAddress: String, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val setup = remember(context) { CompanionSetup(context) }
    if (!setup.isAvailable) return
    var step by rememberSaveable { mutableStateOf(OfferStep.ASK) }
    if (step == OfferStep.SKIPPED) return

    val chooser = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            setup.startObserving(bluetoothAddress)
            step = OfferStep.DONE
        } else {
            step = OfferStep.ASK
        }
    }
    val listener = remember(setup, bluetoothAddress) {
        object : CompanionSetup.Listener {
            override fun onChooser(sender: IntentSender) {
                chooser.launch(IntentSenderRequest.Builder(sender).build())
            }

            override fun onAssociated() {
                setup.startObserving(bluetoothAddress)
                step = OfferStep.DONE
            }

            override fun onFailure(reason: CompanionSetup.Failure) {
                step = if (reason == CompanionSetup.Failure.NOT_FOUND) OfferStep.NOT_FOUND else OfferStep.UNAVAILABLE
            }
        }
    }
    fun begin() {
        step = OfferStep.WORKING
        setup.associate(bluetoothAddress, listener)
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) begin() else step = OfferStep.ASK
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
                onClick = { setup.permission?.let(permission::launch) ?: begin() },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.feature_pairing_companion_action)) }
        }
        if (step != OfferStep.DONE && step != OfferStep.WORKING) {
            OutlinedButton(onClick = { step = OfferStep.SKIPPED }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_companion_skip))
            }
        }
    }
}
