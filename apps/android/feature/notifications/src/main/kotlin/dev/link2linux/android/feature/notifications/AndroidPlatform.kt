package dev.link2linux.android.feature.notifications

import android.app.ActivityManager
import android.app.KeyguardManager
import android.companion.CompanionDeviceManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.os.Build
import android.provider.Settings
import androidx.core.app.NotificationManagerCompat
import dev.link2linux.android.core.notifications.NotificationEnvironment

/** [SettingsStore] on SharedPreferences. */
internal class SharedPreferencesStore(private val prefs: SharedPreferences) : SettingsStore {
    override fun getBoolean(key: String, default: Boolean) = prefs.getBoolean(key, default)

    override fun putBoolean(key: String, value: Boolean) = prefs.edit().putBoolean(key, value).apply()

    override fun getStrings(key: String): Set<String> = prefs.getStringSet(key, emptySet()).orEmpty().toSet()

    override fun putStrings(key: String, value: Set<String>) = prefs.edit().putStringSet(key, value).apply()
}

/** Whether the user granted notification access, and where they do it. */
internal object NotificationAccess {
    fun isGranted(context: Context): Boolean =
        context.packageName in NotificationManagerCompat.getEnabledListenerPackages(context)

    /** The system page for this app's access when there is one (Android 11 and later), else the list of all apps. */
    fun settingsIntent(context: Context): Intent {
        val component = ComponentName(context, L2lNotificationListener::class.java)
        val intent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
                .putExtra(Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME, component.flattenToString())
        } else {
            Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
        }
        return intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

    /** The list of all apps, which always exists; the fallback if the detail page cannot be opened. */
    fun listIntent(): Intent =
        Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}

/**
 * Facts for the engine's NOT_ALLOWED checks.
 *
 * Unverified without a device: that a companion association lifts the
 * background activity start limit for PendingIntents the way the protocol
 * document assumes.
 */
internal class AndroidEnvironment(private val context: Context) : NotificationEnvironment {
    override val isLocked: Boolean
        get() = context.getSystemService(KeyguardManager::class.java)?.isKeyguardLocked ?: true

    override val canStartActivitiesFromBackground: Boolean
        get() = isInForeground() || hasCompanionAssociation()

    private fun isInForeground(): Boolean {
        val info = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(info)
        return info.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    }

    @Suppress("DEPRECATION")
    private fun hasCompanionAssociation(): Boolean = try {
        val manager = context.getSystemService(CompanionDeviceManager::class.java)
        when {
            manager == null -> false
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU -> manager.myAssociations.isNotEmpty()
            else -> manager.associations.isNotEmpty()
        }
    } catch (_: RuntimeException) {
        false
    }
}
