package dev.link2linux.android.service

import android.companion.AssociationInfo
import android.companion.CompanionDeviceService
import android.companion.DevicePresenceEvent
import android.os.Build
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat

/**
 * Android binds this service when the computer that the phone is associated
 * with comes in range, and starts the app if it is not running. That is one
 * of the few moments an app may start a foreground service from the
 * background, so the connection service starts here, then announces the
 * phone with a beacon burst. A presence event is only a reason to announce:
 * it proves nothing about the computer.
 *
 * Presence observing needs Android 12 or later, so the service does nothing
 * before that. Which callback Android uses depends on its version (the address form
 * before 33, the association form from 33, the event form from 36), so all
 * three lead to the same place. Unverified on a device.
 */
@RequiresApi(31)
class DesktopPresenceService : CompanionDeviceService() {

    @Deprecated("Replaced by the AssociationInfo form on Android 13")
    override fun onDeviceAppeared(address: String) {
        if (Build.VERSION.SDK_INT < 33) desktopAppeared() // 31 and 32
    }

    override fun onDeviceAppeared(associationInfo: AssociationInfo) {
        if (Build.VERSION.SDK_INT in 33..35) desktopAppeared()
    }

    @RequiresApi(36)
    override fun onDevicePresenceEvent(event: DevicePresenceEvent) {
        if (event.event == DevicePresenceEvent.EVENT_BLE_APPEARED) desktopAppeared()
    }

    private fun desktopAppeared() {
        // The connection service then starts its beacon burst, in onStartCommand.
        ContextCompat.startForegroundService(this, ConnectionService.desktopNearby(this))
    }
}
