package dev.link2linux.android.feature.notifications

import android.content.ComponentName
import android.content.Context
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import dev.link2linux.android.core.notifications.ImageStore
import dev.link2linux.android.core.notifications.NotificationCanceller
import dev.link2linux.android.core.notifications.NotificationEngine
import dev.link2linux.android.core.notifications.NotificationMapper
import dev.link2linux.android.core.notifications.NotificationPolicy
import dev.link2linux.android.core.notifications.NotificationSnapshot
import dev.link2linux.android.core.notifications.RedactionDetector
import dev.link2linux.android.core.notifications.RemovalCause
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The process-wide notifications feature: settings, the engine, and the glue
 * to the listener service. The session reaches the engine through [engine]:
 * `engine.attach(transport)` when a session starts, `engine.handle(request,
 * replies, transport)` for each request of the desktop,
 * `engine.detach(transport)` when it ends, and `engine.availabilityChanges`
 * for `FeatureStatus`.
 */
class NotificationsRuntime private constructor(context: Context) {
    private val app = context.applicationContext

    private class Policy(val runtime: NotificationsRuntime) : NotificationPolicy {
        override val enabled get() = runtime.settings.state.value.enabled
        override val accessGranted get() = runtime.connected
        override val dismissSync get() = runtime.settings.state.value.dismissSync

        override fun isExcluded(packageName: String) = packageName in runtime.settings.state.value.excluded

        override fun isContentHidden(packageName: String) = packageName in runtime.settings.state.value.contentHidden
    }

    @Volatile
    private var service: NotificationListenerService? = null

    @Volatile
    private var connected = false

    private val _accessGranted = MutableStateFlow(NotificationAccess.isGranted(app))

    /** Whether the user granted notification access in the system settings (not whether the listener is bound yet). */
    val accessGranted: StateFlow<Boolean> = _accessGranted.asStateFlow()

    val settings = NotificationSettings(
        store = SharedPreferencesStore(app.getSharedPreferences("notifications", Context.MODE_PRIVATE)),
        onChanged = ::policyChanged,
    )

    private val extractor = SnapshotExtractor(app)
    private val images = ImageStore()

    @OptIn(ExperimentalCoroutinesApi::class)
    val engine = NotificationEngine(
        // One thread, so the platform's callbacks are applied in the order they arrived.
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Default.limitedParallelism(1)),
        policy = Policy(this),
        environment = AndroidEnvironment(app),
        mapper = NotificationMapper(images, RedactionDetector(bySystem = RedactionDetector.DEFAULT_BY_SYSTEM + SystemPlaceholders.bySystem())),
        images = images,
        canceller = NotificationCanceller { key ->
            val s = service
            if (s == null) {
                false
            } else {
                s.cancelNotification(key)
                true
            }
        },
        ownPackage = app.packageName,
    )

    /** Re-reads the system's notification access state, for example when the settings screen resumes. */
    fun refreshAccess() {
        _accessGranted.value = NotificationAccess.isGranted(app)
        requestRebindIfNeeded()
    }

    private fun policyChanged() {
        engine.onPolicyChanged()
        requestRebindIfNeeded()
    }

    /** Access is granted and the feature is on, but the system has not bound the listener: ask it to. */
    private fun requestRebindIfNeeded() {
        if (!connected && settings.state.value.enabled && NotificationAccess.isGranted(app)) {
            NotificationListenerService.requestRebind(ComponentName(app, L2lNotificationListener::class.java))
        }
    }

    // Called by the listener service.

    internal fun onListenerConnected(service: NotificationListenerService) {
        this.service = service
        connected = true
        _accessGranted.value = true
        extractor.clearCaches()
        val ranking = service.currentRanking
        val snapshots = try {
            service.activeNotifications.orEmpty().map { extractor.extract(it, rankingOf(ranking, it)) }
        } catch (_: SecurityException) {
            emptyList()
        }
        snapshots.forEach(::remember)
        engine.onListenerConnected(snapshots)
    }

    internal fun onListenerDisconnected() {
        service = null
        connected = false
        _accessGranted.value = NotificationAccess.isGranted(app)
        engine.onListenerDisconnected()
    }

    internal fun onPosted(sbn: StatusBarNotification, ranking: NotificationListenerService.RankingMap) {
        val snapshot = extractor.extract(sbn, rankingOf(ranking, sbn))
        remember(snapshot)
        engine.onPosted(snapshot)
    }

    internal fun onRemoved(sbn: StatusBarNotification, reason: Int) {
        engine.onRemoved(sbn.key, RemovalCause.fromAndroidReason(reason))
    }

    private fun rankingOf(map: NotificationListenerService.RankingMap, sbn: StatusBarNotification) =
        NotificationListenerService.Ranking().takeIf { map.getRanking(sbn.key, it) }

    /** Lists the app in the filter screen once it has posted something we could share. */
    private fun remember(s: NotificationSnapshot) {
        if (s.packageName != app.packageName && !s.isFromOtherProfile) settings.rememberApp(s.packageName, s.appName)
    }

    companion object {
        @Volatile
        private var instance: NotificationsRuntime? = null

        fun get(context: Context): NotificationsRuntime =
            instance ?: synchronized(this) {
                instance ?: NotificationsRuntime(context).also { instance = it }
            }
    }
}
