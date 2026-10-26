package dev.link2linux.android.feature.pairing

import androidx.camera.compose.CameraXViewfinder
import androidx.camera.core.Camera
import androidx.camera.core.CameraInfoUnavailableException
import androidx.camera.core.CameraSelector
import androidx.camera.core.CameraState
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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.awaitCancellation
import zxingcpp.BarcodeReader
import java.util.concurrent.Executors

/**
 * The camera preview with a QR reader. Frames are read on a background
 * thread by zxing-cpp and never stored or sent anywhere; [onCode] gets the
 * text of each QR code seen, at most about twice a second, and the caller
 * decides whether it is a pairing code. Needs the camera permission, which
 * the caller has already obtained. It uses the back camera, or the front one
 * on a device without one; [onUnavailable] says there is no camera or it
 * cannot start, so the caller can offer pairing without it. Unverified on a
 * device.
 */
@Composable
internal fun QrScanner(onCode: (String) -> Unit, onUnavailable: () -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val currentOnCode by rememberUpdatedState(onCode)
    val currentOnUnavailable by rememberUpdatedState(onUnavailable)
    var surfaceRequest by remember { mutableStateOf<SurfaceRequest?>(null) }

    LaunchedEffect(lifecycleOwner) {
        val provider = try {
            ProcessCameraProvider.awaitInstance(context)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // The camera service could not start, or the device has no camera.
            currentOnUnavailable()
            return@LaunchedEffect
        }
        val selector = cameraOf(provider)
        if (selector == null) {
            currentOnUnavailable()
            return@LaunchedEffect
        }
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
        var camera: Camera? = null
        try {
            provider.unbindAll()
            camera = provider.bindToLifecycle(lifecycleOwner, selector, preview, analysis)
            // Turned off by a policy, or failed for good: it would only show a black box.
            camera.cameraInfo.cameraState.observe(lifecycleOwner) { state ->
                if (state.error?.type == CameraState.ErrorType.CRITICAL) currentOnUnavailable()
            }
            awaitCancellation()
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // Android refused the camera, or it could not be opened with what the scanner needs.
            currentOnUnavailable()
        } finally {
            camera?.cameraInfo?.cameraState?.removeObservers(lifecycleOwner)
            provider.unbindAll()
            analysis.clearAnalyzer()
            executor.shutdown()
        }
    }

    Box(modifier.clip(RoundedCornerShape(16.dp))) {
        surfaceRequest?.let { CameraXViewfinder(surfaceRequest = it, modifier = Modifier) }
    }
}

/** The back camera, or the front one on a device that has no back camera; null when it has neither. */
private fun cameraOf(provider: ProcessCameraProvider): CameraSelector? =
    listOf(CameraSelector.DEFAULT_BACK_CAMERA, CameraSelector.DEFAULT_FRONT_CAMERA).firstOrNull {
        try {
            provider.hasCamera(it)
        } catch (_: CameraInfoUnavailableException) {
            false
        }
    }
