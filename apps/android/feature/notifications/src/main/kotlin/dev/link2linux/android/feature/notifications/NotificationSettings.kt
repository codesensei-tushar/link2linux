package dev.link2linux.android.feature.notifications

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Small key-value storage, so the settings logic runs in JVM tests. */
interface SettingsStore {
    fun getBoolean(key: String, default: Boolean): Boolean

    fun putBoolean(key: String, value: Boolean)

    fun getStrings(key: String): Set<String>

    fun putStrings(key: String, value: Set<String>)
}

/** What the user chose for the notifications feature. */
data class NotificationSettingsState(
    /** The feature is on. Off until the user turns it on; only then is notification access asked for. */
    val enabled: Boolean = false,
    /** Dismissing on the computer also dismisses on the phone. On by default. */
    val dismissSync: Boolean = true,
    /** Notifications that look like they hold a one-time code are shared without their content. On by default. */
    val hideOneTimeCodes: Boolean = true,
    /** A computer may reply, and press buttons that open an app, while the phone is locked. Off by default. */
    val actWhileLocked: Boolean = false,
    /** Apps whose notifications are not shared. */
    val excluded: Set<String> = emptySet(),
    /** Apps whose notifications are shared without their text. */
    val contentHidden: Set<String> = emptySet(),
    /** Apps seen posting while the feature was on, package to display name, for the filter list. */
    val knownApps: Map<String, String> = emptyMap(),
)

/**
 * The feature's settings. Apps are listed only after they posted a notification,
 * so the app never needs the permission to list every installed app.
 */
class NotificationSettings(private val store: SettingsStore, private val onChanged: () -> Unit = {}) {
    private val _state = MutableStateFlow(load())
    val state: StateFlow<NotificationSettingsState> = _state.asStateFlow()

    fun setEnabled(enabled: Boolean) {
        store.putBoolean(ENABLED, enabled)
        update { copy(enabled = enabled) }
    }

    fun setDismissSync(on: Boolean) {
        store.putBoolean(DISMISS_SYNC, on)
        update { copy(dismissSync = on) }
    }

    fun setHideOneTimeCodes(on: Boolean) {
        store.putBoolean(HIDE_ONE_TIME_CODES, on)
        update { copy(hideOneTimeCodes = on) }
    }

    fun setActWhileLocked(on: Boolean) {
        store.putBoolean(ACT_WHILE_LOCKED, on)
        update { copy(actWhileLocked = on) }
    }

    fun setShared(packageName: String, shared: Boolean) {
        val next = if (shared) _state.value.excluded - packageName else _state.value.excluded + packageName
        store.putStrings(EXCLUDED, next)
        update { copy(excluded = next) }
    }

    fun setContentHidden(packageName: String, hidden: Boolean) {
        val next = if (hidden) _state.value.contentHidden + packageName else _state.value.contentHidden - packageName
        store.putStrings(CONTENT_HIDDEN, next)
        update { copy(contentHidden = next) }
    }

    /** Remembers an app that posted a notification. Does not notify listeners, since nothing is filtered differently. */
    fun rememberApp(packageName: String, name: String) {
        if (_state.value.knownApps[packageName] == name) return
        val next = _state.value.knownApps + (packageName to name)
        _state.value = _state.value.copy(knownApps = next)
        store.putStrings(KNOWN_APPS, next.map { (p, n) -> "$p=$n" }.toSet())
    }

    private inline fun update(change: NotificationSettingsState.() -> NotificationSettingsState) {
        _state.value = _state.value.change()
        onChanged()
    }

    private fun load() = NotificationSettingsState(
        enabled = store.getBoolean(ENABLED, false),
        dismissSync = store.getBoolean(DISMISS_SYNC, true),
        hideOneTimeCodes = store.getBoolean(HIDE_ONE_TIME_CODES, true),
        actWhileLocked = store.getBoolean(ACT_WHILE_LOCKED, false),
        excluded = store.getStrings(EXCLUDED),
        contentHidden = store.getStrings(CONTENT_HIDDEN),
        knownApps = store.getStrings(KNOWN_APPS).mapNotNull {
            val i = it.indexOf('=')
            if (i <= 0) null else it.substring(0, i) to it.substring(i + 1)
        }.toMap(),
    )

    private companion object {
        const val ENABLED = "enabled"
        const val DISMISS_SYNC = "dismiss_sync"
        const val HIDE_ONE_TIME_CODES = "hide_one_time_codes"
        const val ACT_WHILE_LOCKED = "act_while_locked"
        const val EXCLUDED = "excluded"
        const val CONTENT_HIDDEN = "content_hidden"
        const val KNOWN_APPS = "known_apps"
    }
}
