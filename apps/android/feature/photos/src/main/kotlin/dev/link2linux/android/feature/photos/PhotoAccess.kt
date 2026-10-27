package dev.link2linux.android.feature.photos

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat
import dev.link2linux.android.core.photos.PhotoAvailability

/** What the app may read of the phone's photos. */
enum class PhotoAccess {
    NONE,

    /** Only the photos the user picked, which Android allows from version 14. */
    SELECTED,
    ALL,
}

/** The OS permission behind the `photos` permission token. */
object PhotoPermissions {

    /** The permissions to ask for on Android [sdk]: each only on the versions that have it. */
    @SuppressLint("InlinedApi")
    fun requested(sdk: Int = Build.VERSION.SDK_INT): Array<String> = when {
        sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE ->
            arrayOf(Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED)
        sdk >= Build.VERSION_CODES.TIRAMISU -> arrayOf(Manifest.permission.READ_MEDIA_IMAGES)
        else -> arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)
    }

    /** What the app may read now. */
    fun access(context: Context): PhotoAccess = accessOf(Build.VERSION.SDK_INT) {
        ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
    }

    /**
     * What the app may read on Android [sdk] with the permissions [granted]
     * says it holds. From Android 14 on, a user who picked some photos leaves
     * READ_MEDIA_IMAGES denied and grants READ_MEDIA_VISUAL_USER_SELECTED.
     */
    internal fun accessOf(sdk: Int, granted: (String) -> Boolean): PhotoAccess = when {
        sdk >= Build.VERSION_CODES.TIRAMISU && granted(Manifest.permission.READ_MEDIA_IMAGES) -> PhotoAccess.ALL
        sdk >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE && granted(Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED) ->
            PhotoAccess.SELECTED
        sdk < Build.VERSION_CODES.TIRAMISU && granted(Manifest.permission.READ_EXTERNAL_STORAGE) -> PhotoAccess.ALL
        else -> PhotoAccess.NONE
    }

    /** The feature's state for `FeatureStatus`: off until the user turns it on, then waiting for the permission. */
    internal fun availabilityOf(enabled: Boolean, access: PhotoAccess): PhotoAvailability = when {
        !enabled -> PhotoAvailability.DISABLED
        access == PhotoAccess.NONE -> PhotoAvailability.PERMISSION_REQUIRED
        else -> PhotoAvailability.READY
    }
}
