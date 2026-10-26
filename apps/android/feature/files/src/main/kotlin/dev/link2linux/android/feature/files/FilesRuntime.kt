package dev.link2linux.android.feature.files

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.util.Log
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.link2linux.android.core.data.loggingUncaught
import dev.link2linux.android.core.files.FileEngine
import dev.link2linux.android.core.files.FilePeer
import dev.link2linux.android.core.files.FilePolicy
import dev.link2linux.android.core.files.OutgoingFile
import dev.link2linux.android.core.files.TransferKey
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.ConcurrentHashMap
import javax.inject.Inject
import javax.inject.Singleton

/**
 * The process-wide files feature: the user's setting, the engine, and its
 * Android side (Downloads, the documents the user sends, notifications).
 * The session reaches the engine through [engine]; the screens and the
 * share target use [send].
 *
 * Transfers run in the app's process, tied to the sessions with desktops,
 * so they keep going while the connection service runs.
 */
@Singleton
class FilesRuntime @Inject internal constructor(@ApplicationContext context: Context) {
    private val app = context.applicationContext

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + loggingUncaught("the file transfers"))

    val settings = FileSettings(
        store = SharedPreferencesFileStore(app.getSharedPreferences("files", Context.MODE_PRIVATE)),
        onChanged = ::policyChanged,
    )

    private val _storageGranted = MutableStateFlow(StorageAccess.isGranted(app))

    /** Whether received files can be saved: always on Android 10 and later. */
    val storageGranted: StateFlow<Boolean> = _storageGranted.asStateFlow()

    private val policy = object : FilePolicy {
        override val enabled get() = settings.enabled.value
        override val storageGranted get() = StorageAccess.isGranted(app)
    }

    val engine = FileEngine(
        scope = scope,
        policy = policy,
        store = Downloads.store(app),
        onError = { Log.w(TAG, "File transfer failed", it) },
    )

    /** One call of [send]: what its transfers read, let go of once they all ended. */
    private class Sent(val keys: List<TransferKey>, val files: List<OutgoingFile>, val grants: Int?)

    private val sent = ConcurrentHashMap.newKeySet<Sent>()

    /** Documents whose read grant the app keeps, see [keepReadAccess]. */
    private val persisted = ConcurrentHashMap.newKeySet<Uri>()

    private val cacheCleared: Deferred<Unit> = scope.async { OutgoingFiles.clearCache(app) }

    init {
        // One collector, so quick toggles always end with the share target matching the setting.
        scope.launch { settings.enabled.collect(::showShareTarget) }
        TransferNotifier(app, engine, scope).start()
        scope.launch {
            while (true) {
                delay(SWEEP_MILLIS)
                engine.sweep()
            }
        }
        scope.launch {
            engine.transfers.collect { transfers ->
                val running = transfers.filter { !it.state.isFinished }.mapTo(HashSet()) { it.key }
                sent.filter { batch -> batch.keys.none(running::contains) }.forEach { batch ->
                    if (sent.remove(batch)) release(batch)
                }
            }
        }
    }

    /**
     * Sends the documents [uris] to [peer]: now when it is connected and
     * ready, or once it is. Returns how many of them could not be read.
     *
     * @param shared the URIs came with a share, whose read grant lasts only
     *   as long as the activity that received it. Call this while it runs.
     */
    fun send(peer: FilePeer, uris: List<Uri>, shared: Boolean = false): Deferred<Int> {
        val grants = if (shared) ReadGrants.hold(app, uris) else null
        return scope.async {
            cacheCleared.await()
            val resolved = OutgoingFiles.resolve(app, uris)
            val keys = if (resolved.files.isEmpty()) emptyList() else engine.send(peer, resolved.files)
            val batch = Sent(keys, resolved.files, grants)
            sent += batch
            // The transfers may have ended already, with no change of the list left to notice it.
            val running = engine.transfers.value.filter { !it.state.isFinished }.map { it.key }
            if (keys.none(running::contains) && sent.remove(batch)) release(batch)
            Log.d(TAG, "Sending ${resolved.files.size} files, ${resolved.unreadable} unreadable")
            resolved.unreadable
        }
    }

    /**
     * Stops a transfer. It runs in the app's scope, not the caller's: a
     * screen that closes while the cancel waits for a chunk being written
     * must not leave the transfer half cancelled.
     */
    fun cancel(key: TransferKey) {
        scope.launch { engine.cancel(key) }
    }

    /** Re-reads the storage permission, for example when the screen resumes. */
    fun refreshStorageAccess() {
        val granted = StorageAccess.isGranted(app)
        if (_storageGranted.value != granted) {
            _storageGranted.value = granted
            engine.onPolicyChanged()
        }
    }

    private fun policyChanged() {
        _storageGranted.value = StorageAccess.isGranted(app)
        engine.onPolicyChanged()
    }

    /** Lists "Send to computer" in the share sheet only while the feature is on. */
    private fun showShareTarget(on: Boolean) {
        val component = ComponentName(app, SendFilesActivity::class.java)
        val wanted = if (on) PackageManager.COMPONENT_ENABLED_STATE_ENABLED else PackageManager.COMPONENT_ENABLED_STATE_DEFAULT
        val packages = app.packageManager
        if (packages.getComponentEnabledSetting(component) != wanted) {
            packages.setComponentEnabledSetting(component, wanted, PackageManager.DONT_KILL_APP)
        }
    }

    /** Lets go of what a send's transfers read: read grants and cached copies. */
    private fun release(batch: Sent) {
        batch.grants?.let { ReadGrants.release(app, it) }
        batch.files.forEach { file ->
            when (file) {
                is ContentFile -> if (file.uri in persisted) {
                    persisted -= file.uri
                    runCatching { app.contentResolver.releasePersistableUriPermission(file.uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
                }
                is CachedFile -> file.file.delete()
            }
        }
    }

    /**
     * Keeps the read grant of documents the user picked, so a transfer can
     * read them again when it resumes after a reconnect. Shared documents
     * come with a grant that cannot be kept, and are read while it lasts.
     */
    fun keepReadAccess(uris: List<Uri>) {
        uris.forEach { uri ->
            val kept = runCatching {
                app.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }.isSuccess
            if (kept) persisted += uri
        }
    }

    private companion object {
        const val TAG = "link2linux"

        /** How often transfers kept for a reconnect are checked for expiry. */
        const val SWEEP_MILLIS = 60_000L
    }
}
