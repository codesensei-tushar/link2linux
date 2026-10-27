package dev.link2linux.android.feature.status

import android.app.NotificationManager
import android.media.AudioManager
import android.telephony.ServiceState
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import dev.link2linux.android.core.status.MAX_SIGNAL_LEVEL
import dev.link2linux.protocol.v1.Battery
import dev.link2linux.protocol.v1.Cellular
import dev.link2linux.protocol.v1.CellularNetwork
import dev.link2linux.protocol.v1.PhoneStatus
import dev.link2linux.protocol.v1.RingerMode
import dev.link2linux.protocol.v1.Wifi

/**
 * What the platform told so far, each listener its own part, in Android's
 * terms. Null is a part not read yet, or one Android does not tell this app
 * without a permission it does not ask for.
 */
internal data class StatusReadings(
    /** `BatteryManager.EXTRA_LEVEL` and `EXTRA_SCALE`. */
    val batteryLevel: Int? = null,
    val batteryScale: Int? = null,
    /** `BatteryManager.EXTRA_PLUGGED`: 0 on battery, else the kind of power. */
    val plugged: Int = 0,
    val powerSaver: Boolean = false,
    /** Whether the phone has a cellular radio at all. */
    val telephony: Boolean = false,
    /** `SignalStrength.getLevel()`, 0 to 4. */
    val cellLevel: Int = 0,
    /** `ServiceState.getState()`. */
    val serviceState: Int? = null,
    /** A `TelephonyManager.NETWORK_TYPE_*`: the one the status bar shows, or the data registration's. */
    val networkType: Int? = null,
    /** `TelephonyDisplayInfo.getOverrideNetworkType()`, such as 5G on a 4G anchor. */
    val overrideNetworkType: Int? = null,
    val operatorName: String = "",
    /** The best Wi-Fi network's level, 0 to 4, or null when not on Wi-Fi or its level is unknown. */
    val wifiLevel: Int? = null,
    /** `AudioManager.getRingerMode()`. */
    val ringerMode: Int? = null,
    /** `NotificationManager.getCurrentInterruptionFilter()`. */
    val interruptionFilter: Int? = null,
) {

    fun toStatus(): PhoneStatus = PhoneStatus.newBuilder().apply {
        batteryPercent()?.let { percent ->
            battery = Battery.newBuilder()
                .setPercent(percent)
                .setPluggedIn(plugged != 0)
                .setPowerSaver(powerSaver)
                .build()
        }
        if (telephony) {
            cellular = Cellular.newBuilder()
                .setLevel(cellLevel.coerceIn(0, MAX_SIGNAL_LEVEL))
                .setNetwork(network())
                .setOperatorName(operatorName)
                .build()
        }
        wifiLevel?.let { wifi = Wifi.newBuilder().setLevel(it).build() }
        ringerMode = when (this@StatusReadings.ringerMode) {
            AudioManager.RINGER_MODE_NORMAL -> RingerMode.RINGER_MODE_NORMAL
            AudioManager.RINGER_MODE_VIBRATE -> RingerMode.RINGER_MODE_VIBRATE
            AudioManager.RINGER_MODE_SILENT -> RingerMode.RINGER_MODE_SILENT
            else -> RingerMode.RINGER_MODE_UNSPECIFIED
        }
        // Every filter but "all" holds some sounds back.
        doNotDisturb = when (interruptionFilter) {
            NotificationManager.INTERRUPTION_FILTER_PRIORITY,
            NotificationManager.INTERRUPTION_FILTER_ALARMS,
            NotificationManager.INTERRUPTION_FILTER_NONE,
            -> true
            else -> false
        }
    }.build()

    // As the status bar counts it.
    private fun batteryPercent(): Int? {
        val level = batteryLevel ?: return null
        val scale = batteryScale ?: return null
        if (level < 0 || scale <= 0) return null
        return (level * 100 / scale).coerceIn(0, 100)
    }

    private fun network(): CellularNetwork {
        if (serviceState != null && serviceState != ServiceState.STATE_IN_SERVICE) return CellularNetwork.CELLULAR_NETWORK_NO_SERVICE
        return when (overrideNetworkType) {
            TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA,
            TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_ADVANCED,
            -> CellularNetwork.CELLULAR_NETWORK_5G
            else -> generationOf(networkType)
        }
    }

    private fun generationOf(networkType: Int?): CellularNetwork = when (networkType) {
        TelephonyManager.NETWORK_TYPE_GPRS,
        TelephonyManager.NETWORK_TYPE_EDGE,
        TelephonyManager.NETWORK_TYPE_CDMA,
        TelephonyManager.NETWORK_TYPE_1xRTT,
        TelephonyManager.NETWORK_TYPE_IDEN,
        TelephonyManager.NETWORK_TYPE_GSM,
        -> CellularNetwork.CELLULAR_NETWORK_2G
        TelephonyManager.NETWORK_TYPE_UMTS,
        TelephonyManager.NETWORK_TYPE_EVDO_0,
        TelephonyManager.NETWORK_TYPE_EVDO_A,
        TelephonyManager.NETWORK_TYPE_EVDO_B,
        TelephonyManager.NETWORK_TYPE_HSDPA,
        TelephonyManager.NETWORK_TYPE_HSUPA,
        TelephonyManager.NETWORK_TYPE_HSPA,
        TelephonyManager.NETWORK_TYPE_HSPAP,
        TelephonyManager.NETWORK_TYPE_EHRPD,
        TelephonyManager.NETWORK_TYPE_TD_SCDMA,
        -> CellularNetwork.CELLULAR_NETWORK_3G
        TelephonyManager.NETWORK_TYPE_LTE -> CellularNetwork.CELLULAR_NETWORK_4G
        TelephonyManager.NETWORK_TYPE_NR -> CellularNetwork.CELLULAR_NETWORK_5G
        // Unknown, or Wi-Fi calling, which shows no generation.
        else -> CellularNetwork.CELLULAR_NETWORK_UNSPECIFIED
    }

    companion object {
        /** A Wi-Fi level of 0 to [max], as Android counts it, in the protocol's 0 to 4. */
        fun wifiLevel(level: Int, max: Int): Int? =
            if (max <= 0 || level < 0) null else (level * MAX_SIGNAL_LEVEL / max).coerceAtMost(MAX_SIGNAL_LEVEL)
    }
}
