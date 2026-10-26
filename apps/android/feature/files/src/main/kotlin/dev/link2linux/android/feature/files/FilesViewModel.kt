package dev.link2linux.android.feature.files

import android.net.Uri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.link2linux.android.core.data.DesktopConnections
import dev.link2linux.android.core.data.PairedDeviceRepository
import dev.link2linux.android.core.files.FilePeer
import dev.link2linux.android.core.files.Transfer
import dev.link2linux.android.core.files.TransferKey
import dev.link2linux.android.core.files.toFilePeer
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import javax.inject.Inject

/** A paired desktop to send files to. */
data class DesktopChoice(val peer: FilePeer, val connected: Boolean)

/** What came of a share: the desktop it went to, and how many of its files could not be read. */
data class ShareResult(val desktop: DesktopChoice, val unreadable: Int, val total: Int)

/** The files feature for the Files screen and the share target. */
@HiltViewModel
class FilesViewModel @Inject internal constructor(
    private val runtime: FilesRuntime,
    pairedDevices: PairedDeviceRepository,
    connections: DesktopConnections,
) : ViewModel() {

    val enabled: StateFlow<Boolean> = runtime.settings.enabled
    val storageGranted: StateFlow<Boolean> = runtime.storageGranted
    val transfers: StateFlow<List<Transfer>> = runtime.engine.transfers

    /** The paired desktops, the connected ones first; null until they are known. */
    val desktops: StateFlow<List<DesktopChoice>?> =
        combine(pairedDevices.pairedDevices, connections.connected) { devices, connected ->
            devices.map { DesktopChoice(it.toFilePeer(), connected.any(it::accepts)) }.sortedByDescending { it.connected }
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(STOP_MILLIS), null)

    fun setEnabled(enabled: Boolean) = runtime.settings.setEnabled(enabled)

    fun refreshStorageAccess() = runtime.refreshStorageAccess()

    /** Sends [uris] to [peer]; the result is how many could not be read. See [FilesRuntime.send]. */
    fun send(peer: FilePeer, uris: List<Uri>, shared: Boolean): Deferred<Int> = runtime.send(peer, uris, shared)

    fun keepReadAccess(uris: List<Uri>) = runtime.keepReadAccess(uris)

    private val _shareStarted = MutableStateFlow(false)

    /** The share target handed its files over, or is doing so. */
    val shareStarted: StateFlow<Boolean> = _shareStarted.asStateFlow()

    private val _shareResult = MutableStateFlow<ShareResult?>(null)
    val shareResult: StateFlow<ShareResult?> = _shareResult.asStateFlow()

    /**
     * Sends a share's [uris] to [desktop], once: this model outlives the
     * share activity being recreated, while a composition would send again.
     */
    fun share(desktop: DesktopChoice, uris: List<Uri>) {
        if (!_shareStarted.compareAndSet(expect = false, update = true)) return
        val sending = runtime.send(desktop.peer, uris, shared = true)
        viewModelScope.launch { _shareResult.value = ShareResult(desktop, sending.await(), uris.size) }
    }

    fun cancel(key: TransferKey) = runtime.cancel(key)

    fun dismiss(key: TransferKey) = runtime.engine.dismiss(key)

    private companion object {
        const val STOP_MILLIS = 5_000L
    }
}
