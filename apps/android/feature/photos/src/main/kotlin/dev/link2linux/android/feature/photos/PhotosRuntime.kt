package dev.link2linux.android.feature.photos

import android.content.Context
import android.util.Log
import androidx.core.content.edit
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.photos.PhotoEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The process-wide photos feature: the setting, the permission, and the
 * engine. The session reaches the engine through [engine] (see
 * PhotoSessionHooks).
 *
 * Android ends the app's process when the user takes a permission back in
 * its settings, so a new process reads it afresh. A change that leaves the
 * process running, such as the photos the user picked for the app, is read
 * again with [refreshAccess]: when the screen comes back, and when Android
 * refused a read.
 */
@Singleton
class PhotosRuntime @Inject internal constructor(@ApplicationContext context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("photos", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + loggingUncaught("the photos"))

    private val _enabled = MutableStateFlow(prefs.getBoolean(ENABLED, Feature.PHOTOS.isOnByDefault))

    /** Whether the user has the feature on: off until turned on. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    private val _access = MutableStateFlow(PhotoPermissions.access(app))

    /** What the app may read of the phone's photos. */
    val access: StateFlow<PhotoAccess> = _access.asStateFlow()

    val engine = PhotoEngine(
        scope = scope,
        library = AndroidPhotoLibrary(app, onRefused = { refreshAccess() }),
        initialAvailability = PhotoPermissions.availabilityOf(_enabled.value, _access.value),
        // The class only: a failure's message may name a photo.
        onFailure = { Log.w(TAG, "Reading the photos failed: ${it.javaClass.simpleName}") },
    )

    fun setEnabled(on: Boolean) {
        prefs.edit { putBoolean(ENABLED, on) }
        _enabled.value = on
        update()
    }

    /**
     * Reads the permission again. [changedPick] says the user may have
     * picked other photos, which changes the list without changing the
     * permission.
     */
    fun refreshAccess(changedPick: Boolean = false) {
        val before = _access.value
        _access.value = PhotoPermissions.access(app)
        update()
        if (changedPick || _access.value != before) engine.refresh()
    }

    private fun update() = engine.setAvailability(PhotoPermissions.availabilityOf(_enabled.value, _access.value))

    private companion object {
        const val TAG = "link2linux"
        const val ENABLED = "enabled"
    }
}
