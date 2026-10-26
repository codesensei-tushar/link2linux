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
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import javax.inject.Inject

/** A paired desktop to send files to. */
data class DesktopChoice(val peer: FilePeer, val connected: Boolean)

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

    /** Sends documents the user picked to [peer]; the result is how many could not be read. See [FilesRuntime.send]. */
    fun send(peer: FilePeer, uris: List<Uri>): Deferred<Int> = runtime.send(peer, uris)

    private val shareFlow = ShareFlow<Uri, FilesRuntime.Prepared, DesktopChoice>(
        scope = viewModelScope,
        read = runtime::prepareShare,
        send = { desktop, prepared -> runtime.sendShare(desktop.peer, prepared) },
        discard = runtime::discard,
    )

    /**
     * The share target's share. It lives here, which outlives the activity
     * being recreated, so the documents are read once and sent at most once.
     */
    val share: StateFlow<ShareState<FilesRuntime.Prepared, DesktopChoice>> = shareFlow.state

    /** Reads the shared [uris], for the user to confirm. */
    fun prepareShare(uris: List<Uri>) = shareFlow.prepare(uris)

    fun confirmShare(desktop: DesktopChoice) = shareFlow.confirm(desktop)

    fun cancelShare() = shareFlow.cancel()

    override fun onCleared() = shareFlow.close()

    fun cancel(key: TransferKey) = runtime.cancel(key)

    fun dismiss(key: TransferKey) = runtime.engine.dismiss(key)

    private companion object {
        const val STOP_MILLIS = 5_000L
    }
}
