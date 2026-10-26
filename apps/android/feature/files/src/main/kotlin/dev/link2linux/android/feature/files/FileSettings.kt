package dev.link2linux.android.feature.files

import android.Manifest
import android.content.Context
import android.content.SharedPreferences
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Small key-value storage, so the settings logic runs in JVM tests. */
interface FileSettingsStore {
    fun getBoolean(key: String, default: Boolean): Boolean

    fun putBoolean(key: String, value: Boolean)
}

internal class SharedPreferencesFileStore(private val prefs: SharedPreferences) : FileSettingsStore {
    override fun getBoolean(key: String, default: Boolean) = prefs.getBoolean(key, default)

    override fun putBoolean(key: String, value: Boolean) = prefs.edit { putBoolean(key, value) }
}

/**
 * Whether the user turned files on. Off until they do: until then the
 * phone neither sends nor takes files, and on Android 8 and 9 it asks for
 * storage access only when the user turns it on.
 */
class FileSettings(private val store: FileSettingsStore, private val onChanged: () -> Unit = {}) {
    private val _enabled = MutableStateFlow(store.getBoolean(ENABLED, false))
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    fun setEnabled(enabled: Boolean) {
        store.putBoolean(ENABLED, enabled)
        _enabled.value = enabled
        onChanged()
    }

    private companion object {
        const val ENABLED = "enabled"
    }
}

/**
 * Shared storage access. Android 10 and later let the app add files to
 * Downloads without a permission; Android 8 and 9 need
 * WRITE_EXTERNAL_STORAGE, which the manifest asks for on those versions
 * only.
 */
object StorageAccess {
    val needsPermission: Boolean get() = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q

    const val PERMISSION = Manifest.permission.WRITE_EXTERNAL_STORAGE

    fun isGranted(context: Context): Boolean =
        !needsPermission || ContextCompat.checkSelfPermission(context, PERMISSION) == PackageManager.PERMISSION_GRANTED
}
