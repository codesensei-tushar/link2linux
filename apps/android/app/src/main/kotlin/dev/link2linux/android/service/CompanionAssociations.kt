package dev.link2linux.android.service

import android.annotation.SuppressLint
import android.companion.CompanionDeviceManager
import android.companion.ObservingDevicePresenceRequest
import android.content.Context
import android.os.Build
import dev.link2linux.android.core.model.PairedDevice
import kotlinx.coroutines.flow.Flow

/**
 * Removes the companion-device associations that no pairing needs any
 * more. An association lets Android wake the app when its computer comes in
 * range and grants the app background exemptions, so once the computer is
 * unpaired it must go too: the user unpaired it on the phone, the desktop
 * said it was unpaired, or a pairing with another key replaced it.
 */
internal class CompanionAssociations(context: Context) {

    private val manager: CompanionDeviceManager? = context.getSystemService(CompanionDeviceManager::class.java)

    /** Follows the pairings and removes associations as their pairings go. Returns when [pairings] ends. */
    suspend fun follow(pairings: Flow<List<PairedDevice>>) {
        if (manager == null) return
        var before: List<PairedDevice>? = null
        pairings.collect { now ->
            unneeded(before, now, associated()).forEach(::remove)
            before = now
        }
    }

    /** The Bluetooth addresses of this app's associations. */
    private fun associated(): List<String> {
        val manager = manager ?: return emptyList()
        return try {
            if (Build.VERSION.SDK_INT >= 33) {
                manager.myAssociations.mapNotNull { it.deviceMacAddress?.toString() }
            } else {
                @Suppress("DEPRECATION")
                manager.associations
            }
        } catch (_: RuntimeException) {
            emptyList()
        }
    }

    // The address forms are deprecated from Android 13 (disassociate) and 16 (presence), where they are not used.
    @Suppress("DEPRECATION")
    @SuppressLint("MissingPermission")
    private fun remove(address: String) {
        val manager = manager ?: return
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                manager.myAssociations
                    .filter { it.deviceMacAddress?.toString().equals(address, ignoreCase = true) }
                    .forEach { association ->
                        if (Build.VERSION.SDK_INT >= 36) {
                            manager.stopObservingDevicePresence(
                                ObservingDevicePresenceRequest.Builder().setAssociationId(association.id).build(),
                            )
                        } else {
                            manager.stopObservingDevicePresence(address)
                        }
                        manager.disassociate(association.id)
                    }
            } else {
                if (Build.VERSION.SDK_INT >= 31) manager.stopObservingDevicePresence(address)
                manager.disassociate(address)
            }
        } catch (_: RuntimeException) {
            // Already gone, or Android refused.
        }
    }

    companion object {
        /**
         * Which of the [associated] addresses no pairing needs any more, now
         * that the pairings went from [before] (null at the start) to [now]:
         * all of them when nothing is paired, otherwise those of desktops
         * whose pairing went and that no remaining pairing has. A pairing
         * that only lost its Bluetooth address keeps its association: the
         * desktop may announce the address again.
         */
        fun unneeded(before: List<PairedDevice>?, now: List<PairedDevice>, associated: List<String>): List<String> {
            if (now.isEmpty()) return associated
            val kept = now.mapNotNullTo(mutableSetOf()) { it.bluetoothAddress?.uppercase() }
            val gone = before.orEmpty()
                .filter { old -> now.none { it.device.id == old.device.id } }
                .mapNotNullTo(mutableSetOf()) { it.bluetoothAddress?.uppercase() } - kept
            return associated.filter { it.uppercase() in gone }
        }
    }
}
