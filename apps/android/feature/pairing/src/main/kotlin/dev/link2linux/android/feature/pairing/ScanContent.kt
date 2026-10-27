package dev.link2linux.android.feature.pairing

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
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
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import dev.link2linux.android.core.designsystem.component.L2lButton
import dev.link2linux.android.core.designsystem.component.L2lCallout
import dev.link2linux.android.core.designsystem.component.L2lOutlinedButton
import dev.link2linux.android.core.designsystem.component.Tone

private enum class CameraStep { RATIONALE, DENIED }

/**
 * The scanning screen. The phone asks for the camera only when the user taps
 * "Allow camera" after reading why, never on its own. A denial leads to a
 * screen that offers the phone's settings and pairing without the camera,
 * and so does a camera that cannot start.
 */
@Composable
internal fun ScanContent(
    scanError: ScanError?,
    actions: PairingActions,
    modifier: Modifier,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    // Read again on each resume, never restored: the user can turn the camera off in the settings while away.
    var granted by remember { mutableStateOf(hasCameraPermission(context)) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { granted = hasCameraPermission(context) }
    var step by rememberSaveable { mutableStateOf(CameraStep.RATIONALE) }
    var cameraFailed by rememberSaveable { mutableStateOf(false) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { isGranted ->
        granted = isGranted
        if (!isGranted) step = CameraStep.DENIED
    }

    when {
        granted && cameraFailed -> CameraMessage(
            title = stringResource(R.string.feature_pairing_camera_unavailable_title),
            body = stringResource(R.string.feature_pairing_camera_unavailable_body),
            calloutTone = Tone.Warning,
            modifier = modifier.padding(padding),
        ) {
            L2lButton(onClick = actions.onPairWithoutCamera, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_sas_action))
            }
            L2lOutlinedButton(onClick = { cameraFailed = false }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_try_again))
            }
        }
        granted -> Column(modifier = modifier.padding(padding)) {
            Text(
                text = stringResource(R.string.feature_pairing_scan_title),
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 16.dp),
            )
            L2lCallout(
                text = stringResource(R.string.feature_pairing_scan_body),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp),
            )
            QrScanner(
                onCode = actions.onScanned,
                onUnavailable = { cameraFailed = true },
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f)
                    .padding(16.dp),
            )
            scanError?.let {
                L2lCallout(
                    text = stringResource(
                        if (it == ScanError.UPDATE_THE_APP) {
                            R.string.feature_pairing_scan_error_update
                        } else {
                            R.string.feature_pairing_scan_error_invalid
                        },
                    ),
                    tone = Tone.Danger,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
        step == CameraStep.RATIONALE -> CameraMessage(
            title = stringResource(R.string.feature_pairing_camera_title),
            body = stringResource(R.string.feature_pairing_camera_rationale),
            calloutTone = Tone.Accent,
            modifier = modifier.padding(padding),
        ) {
            L2lButton(onClick = { request.launch(Manifest.permission.CAMERA) }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_camera_allow))
            }
            L2lOutlinedButton(onClick = actions.onPairWithoutCamera, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_sas_action))
            }
        }
        else -> CameraMessage(
            title = stringResource(R.string.feature_pairing_camera_denied_title),
            body = stringResource(R.string.feature_pairing_camera_denied_body),
            calloutTone = Tone.Warning,
            modifier = modifier.padding(padding),
        ) {
            L2lButton(onClick = { openAppSettings(context) }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_camera_settings))
            }
            L2lOutlinedButton(onClick = actions.onPairWithoutCamera, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_sas_action))
            }
            L2lOutlinedButton(onClick = { step = CameraStep.RATIONALE }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_try_again))
            }
        }
    }
}

@Composable
private fun CameraMessage(
    title: String,
    body: String,
    calloutTone: Tone,
    modifier: Modifier,
    buttons: @Composable () -> Unit,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
    ) {
        Text(text = title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
        L2lCallout(text = body, tone = calloutTone, modifier = Modifier.fillMaxWidth())
        Column(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            buttons()
        }
    }
}

private fun hasCameraPermission(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

private fun openAppSettings(context: Context) {
    val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    context.startActivity(intent)
}
