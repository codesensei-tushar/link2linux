package dev.link2linux.android.feature.pairing

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.link2linux.android.core.designsystem.component.L2lButton
import dev.link2linux.android.core.designsystem.component.L2lCallout
import dev.link2linux.android.core.designsystem.component.L2lCard
import dev.link2linux.android.core.designsystem.component.L2lOutlinedButton
import dev.link2linux.android.core.designsystem.component.StatusBadge
import dev.link2linux.android.core.designsystem.component.Tone
import dev.link2linux.android.core.model.PairingFailure

/** The screens of a pairing attempt after the code is scanned or "pair without camera" is on. */
@Composable
internal fun AttemptContent(
    screen: PairingScreenState,
    actions: PairingActions,
    modifier: Modifier,
    padding: PaddingValues,
) {
    Column(
        modifier = modifier
            .padding(padding)
            .padding(horizontal = 16.dp, vertical = 16.dp)
            .verticalScroll(rememberScrollState()),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        when (screen) {
            is PairingScreenState.WaitingForQr -> {
                Title(stringResource(R.string.feature_pairing_waiting_title, screen.desktopName))
                StatusBadge(text = stringResource(R.string.feature_pairing_waiting_badge), tone = Tone.Warning)
                CircularProgressIndicator()
                Body(stringResource(R.string.feature_pairing_waiting_body, screen.secondsLeft))
                if (screen.showAddressHint && screen.endpoints.isNotEmpty()) {
                    L2lCallout(
                        text = stringResource(R.string.feature_pairing_address_hint),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Endpoints(screen.endpoints)
                }
                CancelButton(actions)
            }
            is PairingScreenState.WaitingForSas -> {
                Title(stringResource(R.string.feature_pairing_sas_title))
                StatusBadge(text = stringResource(R.string.feature_pairing_waiting_badge), tone = Tone.Warning)
                val name = screen.instanceName ?: stringResource(R.string.feature_pairing_sas_name_unknown)
                Body(stringResource(R.string.feature_pairing_sas_body, name, screen.secondsLeft))
                Body(stringResource(R.string.feature_pairing_sas_attempts, screen.attemptsLeft))
                screen.lastFailure?.let {
                    L2lCallout(
                        text = stringResource(R.string.feature_pairing_sas_last_failure, stringResource(failureText(it))),
                        tone = Tone.Danger,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (screen.endpoints.isNotEmpty()) {
                    L2lCallout(
                        text = stringResource(R.string.feature_pairing_sas_address_hint),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Endpoints(screen.endpoints)
                }
                CancelButton(actions)
            }
            is PairingScreenState.Verifying -> {
                Title(stringResource(R.string.feature_pairing_verifying_title, screen.desktopName))
                StatusBadge(text = stringResource(R.string.feature_pairing_verifying_badge), tone = Tone.Accent)
                CircularProgressIndicator()
                Body(stringResource(R.string.feature_pairing_verifying_body))
                CancelButton(actions)
            }
            is PairingScreenState.ConfirmCode -> {
                Title(stringResource(R.string.feature_pairing_confirm_title))
                L2lCard(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        text = screen.code,
                        style = MaterialTheme.typography.displayMedium,
                        fontFamily = FontFamily.Monospace,
                        textAlign = TextAlign.Center,
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 16.dp),
                    )
                }
                Body(stringResource(R.string.feature_pairing_confirm_body, screen.desktopName))
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    L2lOutlinedButton(onClick = { actions.onConfirmCode(false) }) {
                        Text(stringResource(R.string.feature_pairing_confirm_differ))
                    }
                    L2lButton(onClick = { actions.onConfirmCode(true) }) {
                        Text(stringResource(R.string.feature_pairing_confirm_match))
                    }
                }
            }
            is PairingScreenState.ConfirmReplace -> {
                Title(stringResource(R.string.feature_pairing_replace_title, screen.existingName))
                L2lCallout(
                    text = stringResource(R.string.feature_pairing_replace_body, screen.desktopName, screen.existingName),
                    tone = Tone.Warning,
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    L2lOutlinedButton(onClick = { actions.onConfirmReplace(false) }) {
                        Text(stringResource(R.string.feature_pairing_replace_no))
                    }
                    L2lButton(onClick = { actions.onConfirmReplace(true) }) {
                        Text(stringResource(R.string.feature_pairing_replace_yes))
                    }
                }
            }
            is PairingScreenState.Paired -> {
                Title(stringResource(R.string.feature_pairing_success_title, screen.device.device.name))
                StatusBadge(text = stringResource(R.string.feature_pairing_paired), tone = Tone.Success)
                Body(stringResource(R.string.feature_pairing_success_body))
                screen.device.bluetoothAddress?.let {
                    L2lCard(modifier = Modifier.fillMaxWidth()) {
                        CompanionOffer(it, screen.device.pairedAtEpochMillis, Modifier.padding(16.dp))
                    }
                }
                L2lButton(onClick = actions.onDismiss) { Text(stringResource(R.string.feature_pairing_done)) }
            }
            is PairingScreenState.Failed -> {
                Title(stringResource(R.string.feature_pairing_failed_title))
                StatusBadge(text = stringResource(R.string.feature_pairing_failed_badge), tone = Tone.Danger)
                L2lCallout(
                    text = stringResource(failureText(screen.reason)),
                    tone = Tone.Danger,
                    modifier = Modifier.fillMaxWidth(),
                )
                L2lButton(onClick = actions.onDismiss) { Text(stringResource(R.string.feature_pairing_try_again)) }
            }
            PairingScreenState.Home, PairingScreenState.Scanning -> Unit
        }
        Spacer(Modifier.height(16.dp))
    }
}

internal fun failureText(reason: PairingFailure): Int = when (reason) {
    PairingFailure.VERIFICATION_FAILED -> R.string.feature_pairing_failed_verification
    PairingFailure.DECLINED -> R.string.feature_pairing_failed_declined
    PairingFailure.TIMED_OUT -> R.string.feature_pairing_failed_timed_out
    PairingFailure.NOT_EXPECTED -> R.string.feature_pairing_failed_not_expected
    PairingFailure.UNSUPPORTED -> R.string.feature_pairing_failed_unsupported
    PairingFailure.CONNECTION_LOST -> R.string.feature_pairing_failed_connection_lost
    PairingFailure.INTERNAL -> R.string.feature_pairing_failed_internal
}

@Composable
private fun Title(text: String) {
    Spacer(Modifier.height(8.dp))
    Text(text = text, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
}

@Composable
private fun Body(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun Endpoints(endpoints: List<String>) {
    endpoints.forEach {
        Text(text = it, style = MaterialTheme.typography.titleMedium, fontFamily = FontFamily.Monospace)
    }
}

@Composable
private fun CancelButton(actions: PairingActions) {
    L2lOutlinedButton(onClick = actions.onCancel) { Text(stringResource(R.string.feature_pairing_cancel)) }
}
