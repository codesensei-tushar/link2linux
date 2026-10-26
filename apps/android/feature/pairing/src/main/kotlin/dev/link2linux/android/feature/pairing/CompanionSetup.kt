package dev.link2linux.android.feature.pairing

import android.annotation.SuppressLint
import android.bluetooth.le.ScanFilter
import android.companion.AssociationInfo
import android.companion.AssociationRequest
import android.companion.BluetoothLeDeviceFilter
import android.companion.CompanionDeviceManager
import android.companion.ObservingDevicePresenceRequest
import android.content.Context
import android.content.IntentSender
import android.content.pm.PackageManager
import android.os.Build
import android.os.ParcelUuid
import dev.link2linux.android.core.discovery.DiscoveryConstants
import java.util.UUID

/**
 * The optional companion-device step after pairing. Associating this phone
 * with the computer's Bluetooth advertisement lets Android start link2linux
 * when the computer comes in range, even from the background, and the app
 * then starts a beacon burst. It is only a way to be found sooner: nothing
 * is exchanged over Bluetooth, and without it the phone relies on its
 * foreground service alone.
 *
 * What Android does here depends on the version and on the phone's
 * Bluetooth controller (presence detection needs Android 12 and offloaded
 * scan filters), and none of it has been run on a device yet.
 */
internal class CompanionSetup(private val context: Context) {

    private val manager: CompanionDeviceManager? = context.getSystemService(CompanionDeviceManager::class.java)

    /** Whether this phone can associate at all. */
    val isAvailable: Boolean
        get() = manager != null &&
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_COMPANION_DEVICE_SETUP)

    /** The runtime permission to ask for right before associating, or null where none is needed. */
    val permission: String?
        get() = if (Build.VERSION.SDK_INT >= 31) android.Manifest.permission.BLUETOOTH_CONNECT else null

    interface Listener {
        /** Android wants to show its chooser: launch [sender]; the user picks there. */
        fun onChooser(sender: IntentSender)

        /** Android made the association without a chooser result to wait for. */
        fun onAssociated()

        fun onFailure(reason: Failure)
    }

    enum class Failure { NOT_FOUND, UNAVAILABLE }

    /**
     * Asks Android to associate with the computer advertising the link2linux
     * service from [address] ("0C:1A:2B:3C:4D:5E", a public address). The
     * request names exactly that device and offers only it.
     */
    fun associate(address: String, listener: Listener) {
        val manager = manager ?: return listener.onFailure(Failure.UNAVAILABLE)
        val filter = ScanFilter.Builder()
            .setServiceUuid(ParcelUuid(UUID.fromString(DiscoveryConstants.BLE_SERVICE_UUID)))
            .setDeviceAddress(address)
            .build()
        val request = AssociationRequest.Builder()
            .addDeviceFilter(BluetoothLeDeviceFilter.Builder().setScanFilter(filter).build())
            .setSingleDevice(true)
            .build()
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                manager.associate(
                    request,
                    Runnable::run,
                    object : CompanionDeviceManager.Callback() {
                        override fun onAssociationPending(intentSender: IntentSender) = listener.onChooser(intentSender)
                        override fun onAssociationCreated(associationInfo: AssociationInfo) = listener.onAssociated()
                        override fun onFailure(error: CharSequence?) = listener.onFailure(Failure.NOT_FOUND)
                    },
                )
            } else {
                @Suppress("DEPRECATION")
                manager.associate(
                    request,
                    object : CompanionDeviceManager.Callback() {
                        @Deprecated("Replaced by onAssociationPending on Android 13")
                        override fun onDeviceFound(chooserLauncher: IntentSender) = listener.onChooser(chooserLauncher)
                        override fun onFailure(error: CharSequence?) = listener.onFailure(Failure.NOT_FOUND)
                    },
                    null,
                )
            }
        } catch (_: RuntimeException) {
            listener.onFailure(Failure.UNAVAILABLE)
        }
    }

    /**
     * Asks Android to watch for the associated computer and wake the app when
     * it appears. Presence observing exists from Android 12; before that the
     * association alone does nothing, which is fine.
     */
    @SuppressLint("MissingPermission")
    fun startObserving(address: String) {
        val manager = manager ?: return
        try {
            if (Build.VERSION.SDK_INT >= 36) {
                val association = manager.myAssociations.firstOrNull {
                    it.deviceMacAddress?.toString().equals(address, ignoreCase = true)
                } ?: return
                manager.startObservingDevicePresence(
                    ObservingDevicePresenceRequest.Builder().setAssociationId(association.id).build(),
                )
            } else if (Build.VERSION.SDK_INT >= 31) {
                @Suppress("DEPRECATION")
                manager.startObservingDevicePresence(address)
            }
        } catch (_: RuntimeException) {
            // Android refused: no matching association, or the permission is missing.
        }
    }
}
