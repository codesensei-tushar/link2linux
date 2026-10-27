package dev.link2linux.android.feature.screen

import android.accessibilityservice.AccessibilityService
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjection
import android.os.PowerManager
import android.util.Log
import androidx.core.content.edit
import dagger.hilt.EntryPoint
import dagger.hilt.InstallIn
import dagger.hilt.android.EntryPointAccessors
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.screen.ScreenEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The process-wide screen feature: the settings, the engine, and the glue
 * to Android's screen capture and accessibility service. The session
 * reaches the engine through [engine] (see ScreenSessionHooks).
 *
 * The user's consent travels from the question ([ScreenAsk]) through
 * [ScreenShareActivity], which shows Android's own question, to
 * [ScreenCaptureService], which takes the capture in the foreground and
 * hands it to the engine.
 */
@Singleton
class ScreenRuntime @Inject internal constructor(@ApplicationContext context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("screen", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + loggingUncaught("the screen sharing"))
    private val keyguard = app.getSystemService(KeyguardManager::class.java)
    private val power = app.getSystemService(PowerManager::class.java)

    private val _enabled = MutableStateFlow(prefs.getBoolean(ENABLED, Feature.SCREEN.isOnByDefault))

    /** The user turned the feature on. Off until then, like every feature that shares what is on the phone. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    private val _controlAllowed = MutableStateFlow(prefs.getBoolean(CONTROL, false))

    /** The user allows the computer to control the phone. Off until turned on, on its own switch. */
    val controlAllowed: StateFlow<Boolean> = _controlAllowed.asStateFlow()

    private val controlService = MutableStateFlow<AccessibilityService?>(null)

    /** Whether Android runs the app's accessibility service, which the user turns on in its settings. */
    val controlServiceOn: StateFlow<Boolean> = controlService.map { it != null }.stateIn(scope, SharingStarted.Eagerly, false)

    internal val asker = ScreenAsk(app)

    val engine = ScreenEngine(
        scope = scope,
        asker = asker,
        encodable = AndroidScreenCapture.encodable(),
        controller = AndroidScreenController { controlService.value },
        enabled = enabled,
        control = combine(controlAllowed, controlService) { allowed, service -> allowed && service != null }
            .stateIn(scope, SharingStarted.Eagerly, false),
        isLocked = { keyguard?.isKeyguardLocked == true || power?.isInteractive == false },
    )

    fun setEnabled(on: Boolean) {
        prefs.edit { putBoolean(ENABLED, on) }
        _enabled.value = on
    }

    fun setControlAllowed(on: Boolean) {
        prefs.edit { putBoolean(CONTROL, on) }
        _controlAllowed.value = on
    }

    /** The user stopped sharing on the phone. */
    fun stop() = engine.stopOnPhone()

    internal fun controlConnected(service: AccessibilityService) {
        controlService.value = service
    }

    internal fun controlDisconnected(service: AccessibilityService) {
        controlService.compareAndSet(service, null)
    }

    /**
     * The user agreed in Android's question for [askId]; [resultCode] and
     * [data] are its answer. Called while the activity that asked is in
     * front, since only then may the app start the capture's foreground
     * service.
     */
    internal fun consented(askId: Long, resultCode: Int, data: Intent) {
        val question = asker.current.value?.takeIf { it.askId == askId }
        if (question == null || !engine.isAsking(askId)) return
        try {
            ScreenCaptureService.start(app, askId, question.desktopName, resultCode, data)
        } catch (e: RuntimeException) {
            // Android refuses a foreground service started from the background, and the activity may have lost its turn.
            Log.w(TAG, "Could not start the screen capture: ${e.javaClass.simpleName}")
            engine.couldNotStart(askId)
        }
    }

    /** The user declined the question [askId], on the phone. */
    internal fun declined(askId: Long) = engine.declined(askId)

    /** The capture service runs in the foreground and holds [projection] for [askId]; [onReleased] stops it. */
    internal fun projectionReady(askId: Long, projection: MediaProjection, onReleased: () -> Unit) {
        engine.granted(askId, AndroidScreenCapture(app, projection, onReleased))
    }

    @EntryPoint
    @InstallIn(SingletonComponent::class)
    internal interface Screen {
        fun screenRuntime(): ScreenRuntime
    }

    internal companion object {
        private const val TAG = "link2linux"
        private const val ENABLED = "enabled"
        private const val CONTROL = "control"

        /** For the activity, services and receiver, which Hilt does not inject. */
        fun from(context: Context): ScreenRuntime =
            EntryPointAccessors.fromApplication(context.applicationContext, Screen::class.java).screenRuntime()
    }
}
