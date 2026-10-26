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
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat

private enum class CameraStep { RATIONALE, DENIED }

/**
 * The scanning screen. The phone asks for the camera only when the user taps
 * "Allow camera" after reading why, never on its own. A denial leads to a
 * screen that offers the phone's settings and pairing without the camera.
 */
@Composable
internal fun ScanContent(
    scanError: ScanError?,
    actions: PairingActions,
    modifier: Modifier,
    padding: PaddingValues,
) {
    val context = LocalContext.current
    var granted by rememberSaveable { mutableStateOf(hasCameraPermission(context)) }
    var step by rememberSaveable { mutableStateOf(CameraStep.RATIONALE) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { isGranted ->
        granted = isGranted
        if (!isGranted) step = CameraStep.DENIED
    }

    when {
        granted -> Column(modifier = modifier.padding(padding)) {
            Text(
                text = stringResource(R.string.feature_pairing_scan_title),
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.padding(horizontal = 24.dp, vertical = 16.dp),
            )
            Text(
                text = stringResource(R.string.feature_pairing_scan_body),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
            QrScanner(
                onCode = actions.onScanned,
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f)
                    .padding(16.dp),
            )
            scanError?.let {
                Text(
                    text = stringResource(
                        if (it == ScanError.UPDATE_THE_APP) {
                            R.string.feature_pairing_scan_error_update
                        } else {
                            R.string.feature_pairing_scan_error_invalid
                        },
                    ),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
                )
            }
        }
        step == CameraStep.RATIONALE -> CameraMessage(
            title = stringResource(R.string.feature_pairing_camera_title),
            body = stringResource(R.string.feature_pairing_camera_rationale),
            modifier = modifier.padding(padding),
        ) {
            Button(onClick = { request.launch(Manifest.permission.CAMERA) }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_camera_allow))
            }
            OutlinedButton(onClick = actions.onPairWithoutCamera, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_sas_action))
            }
        }
        else -> CameraMessage(
            title = stringResource(R.string.feature_pairing_camera_denied_title),
            body = stringResource(R.string.feature_pairing_camera_denied_body),
            modifier = modifier.padding(padding),
        ) {
            Button(onClick = { openAppSettings(context) }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_camera_settings))
            }
            OutlinedButton(onClick = actions.onPairWithoutCamera, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_sas_action))
            }
            OutlinedButton(onClick = { step = CameraStep.RATIONALE }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.feature_pairing_try_again))
            }
        }
    }
}

@Composable
private fun CameraMessage(
    title: String,
    body: String,
    modifier: Modifier,
    buttons: @Composable () -> Unit,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
    ) {
        Text(text = title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
        Text(
            text = body,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        buttons()
    }
}

private fun hasCameraPermission(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

private fun openAppSettings(context: Context) {
    val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    context.startActivity(intent)
}
