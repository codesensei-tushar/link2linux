package dev.link2linux.android.feature.clipboard

import android.annotation.SuppressLint
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import dev.link2linux.android.core.clipboard.ConnectedDesktop
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * The quick settings tile "Send clipboard". A tap opens
 * [ClipboardSendActivity], which takes focus for a moment so it can read
 * the clipboard; on a locked phone the user unlocks first, since Android
 * does not let apps read the clipboard while it is locked. The tile is
 * active while a connected computer takes the clipboard.
 */
class ClipboardTileService : TileService() {

    private var listening: CoroutineScope? = null

    override fun onStartListening() {
        super.onStartListening()
        val runtime = ClipboardRuntime.get(this)
        listening?.cancel()
        listening = MainScope().apply {
            launch { combine(runtime.enabled, runtime.engine.connected) { on, desktops -> update(on, desktops) }.collect() }
        }
    }

    override fun onStopListening() {
        listening?.cancel()
        listening = null
        super.onStopListening()
    }

    override fun onClick() {
        super.onClick()
        if (isLocked) unlockAndRun(::open) else open()
    }

    private fun open() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startActivityAndCollapse(ClipboardSendActivity.pendingIntent(this, 0))
        } else {
            openBeforeAndroid14()
        }
    }

    // Before Android 14 the Intent form is the only one; it is deprecated, and refused, only from Android 14 on.
    @Suppress("DEPRECATION")
    @SuppressLint("StartActivityAndCollapseDeprecated")
    private fun openBeforeAndroid14() {
        startActivityAndCollapse(ClipboardSendActivity.intent(this).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    private fun update(on: Boolean, desktops: List<ConnectedDesktop>) {
        val tile = qsTile ?: return
        val ready = desktops.filter { it.isReady }
        tile.state = if (on && ready.isNotEmpty()) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            tile.subtitle = when {
                !on -> getString(R.string.clipboard_tile_off)
                ready.isEmpty() -> getString(R.string.clipboard_tile_not_connected)
                else -> ready.joinToString(", ") { it.name }
            }
        }
        tile.updateTile()
    }
}
