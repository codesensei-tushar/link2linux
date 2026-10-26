package dev.link2linux.android.feature.pairing

import androidx.camera.compose.CameraXViewfinder
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.lifecycle.awaitInstance
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.awaitCancellation
import zxingcpp.BarcodeReader
import java.util.concurrent.Executors

/**
 * The camera preview with a QR reader. Frames are read on a background
 * thread by zxing-cpp and never stored or sent anywhere; [onCode] gets the
 * text of each QR code seen, at most about twice a second, and the caller
 * decides whether it is a pairing code. Needs the camera permission, which
 * the caller has already obtained. Unverified on a device.
 */
@Composable
internal fun QrScanner(onCode: (String) -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val currentOnCode by rememberUpdatedState(onCode)
    var surfaceRequest by remember { mutableStateOf<SurfaceRequest?>(null) }

    LaunchedEffect(lifecycleOwner) {
        val provider = ProcessCameraProvider.awaitInstance(context)
        val preview = Preview.Builder().build().apply {
            setSurfaceProvider { request -> surfaceRequest = request }
        }
        val executor = Executors.newSingleThreadExecutor()
        val reader = BarcodeReader(
            BarcodeReader.Options(formats = setOf(BarcodeReader.Format.QR_CODE), tryRotate = true, tryInvert = true),
        )
        val mainExecutor = ContextCompat.getMainExecutor(context)
        val analysis = ImageAnalysis.Builder()
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
            .apply {
                setAnalyzer(executor) { image ->
                    image.use {
                        val text = reader.read(it).firstOrNull()?.text
                        if (text != null) mainExecutor.execute { currentOnCode(text) }
                    }
                }
            }
        try {
            provider.unbindAll()
            provider.bindToLifecycle(lifecycleOwner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
            awaitCancellation()
        } finally {
            provider.unbindAll()
            analysis.clearAnalyzer()
            executor.shutdown()
        }
    }

    Box(modifier.clip(RoundedCornerShape(16.dp))) {
        surfaceRequest?.let { CameraXViewfinder(surfaceRequest = it, modifier = Modifier) }
    }
}
