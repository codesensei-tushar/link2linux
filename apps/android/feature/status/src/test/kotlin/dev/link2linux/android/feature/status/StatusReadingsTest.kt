package dev.link2linux.android.feature.status

import android.app.NotificationManager
import android.media.AudioManager
import android.os.BatteryManager
import android.telephony.ServiceState
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import dev.link2linux.protocol.v1.CellularNetwork
import dev.link2linux.protocol.v1.RingerMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class StatusReadingsTest {

    private val inService = StatusReadings(telephony = true, serviceState = ServiceState.STATE_IN_SERVICE)

    @Test
    fun theBatteryIsShownAsTheStatusBarCountsIt() {
        val status = StatusReadings(batteryLevel = 4_321, batteryScale = 10_000, plugged = BatteryManager.BATTERY_PLUGGED_USB, powerSaver = true)
            .toStatus()

        assertEquals(43, status.battery.percent)
        assertTrue(status.battery.pluggedIn)
        assertTrue(status.battery.powerSaver)
    }

    @Test
    fun aBatteryNotReadYetIsLeftOut() {
        assertFalse(StatusReadings().toStatus().hasBattery())
        assertFalse(StatusReadings(batteryLevel = -1, batteryScale = 100).toStatus().hasBattery())
        assertFalse(StatusReadings(batteryLevel = 50, batteryScale = 0).toStatus().hasBattery())
    }

    @Test
    fun aPhoneWithoutTelephonyHasNoCellular() {
        assertFalse(StatusReadings(telephony = false, cellLevel = 3).toStatus().hasCellular())
    }

    @Test
    fun theGenerationIsTheOneTheStatusBarShows() {
        fun network(readings: StatusReadings) = readings.toStatus().cellular.network

        assertEquals(CellularNetwork.CELLULAR_NETWORK_4G, network(inService.copy(networkType = TelephonyManager.NETWORK_TYPE_LTE)))
        assertEquals(
            CellularNetwork.CELLULAR_NETWORK_5G,
            network(
                inService.copy(
                    networkType = TelephonyManager.NETWORK_TYPE_LTE,
                    overrideNetworkType = TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA,
                ),
            ),
        )
        assertEquals(
            CellularNetwork.CELLULAR_NETWORK_4G,
            network(
                inService.copy(
                    networkType = TelephonyManager.NETWORK_TYPE_LTE,
                    overrideNetworkType = TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_LTE_CA,
                ),
            ),
        )
        assertEquals(CellularNetwork.CELLULAR_NETWORK_5G, network(inService.copy(networkType = TelephonyManager.NETWORK_TYPE_NR)))
        assertEquals(CellularNetwork.CELLULAR_NETWORK_3G, network(inService.copy(networkType = TelephonyManager.NETWORK_TYPE_HSPAP)))
        assertEquals(CellularNetwork.CELLULAR_NETWORK_2G, network(inService.copy(networkType = TelephonyManager.NETWORK_TYPE_EDGE)))
        assertEquals(CellularNetwork.CELLULAR_NETWORK_UNSPECIFIED, network(inService))
        assertEquals(CellularNetwork.CELLULAR_NETWORK_UNSPECIFIED, network(inService.copy(networkType = TelephonyManager.NETWORK_TYPE_IWLAN)))
    }

    @Test
    fun noServiceWinsOverTheLastNetwork() {
        for (state in listOf(ServiceState.STATE_OUT_OF_SERVICE, ServiceState.STATE_EMERGENCY_ONLY, ServiceState.STATE_POWER_OFF)) {
            val readings = inService.copy(serviceState = state, networkType = TelephonyManager.NETWORK_TYPE_LTE)
            assertEquals(CellularNetwork.CELLULAR_NETWORK_NO_SERVICE, readings.toStatus().cellular.network)
        }
    }

    @Test
    fun theCellularLevelAndOperatorAreKept() {
        val cellular = inService.copy(cellLevel = 3, operatorName = "Carrier").toStatus().cellular

        assertEquals(3, cellular.level)
        assertEquals("Carrier", cellular.operatorName)
    }

    @Test
    fun wifiIsSetOnlyWithALevel() {
        assertFalse(StatusReadings().toStatus().hasWifi())
        assertEquals(2, StatusReadings(wifiLevel = 2).toStatus().wifi.level)
    }

    @Test
    fun wifiLevelsScaleToFourBars() {
        assertEquals(4, StatusReadings.wifiLevel(4, 4))
        assertEquals(2, StatusReadings.wifiLevel(2, 4))
        assertEquals(4, StatusReadings.wifiLevel(5, 5))
        assertEquals(2, StatusReadings.wifiLevel(3, 5))
        assertEquals(0, StatusReadings.wifiLevel(0, 4))
        assertNull(StatusReadings.wifiLevel(-1, 4))
        assertNull(StatusReadings.wifiLevel(1, 0))
    }

    @Test
    fun soundModes() {
        assertEquals(RingerMode.RINGER_MODE_VIBRATE, StatusReadings(ringerMode = AudioManager.RINGER_MODE_VIBRATE).toStatus().ringerMode)
        assertEquals(RingerMode.RINGER_MODE_SILENT, StatusReadings(ringerMode = AudioManager.RINGER_MODE_SILENT).toStatus().ringerMode)
        assertEquals(RingerMode.RINGER_MODE_UNSPECIFIED, StatusReadings().toStatus().ringerMode)

        assertFalse(StatusReadings(interruptionFilter = NotificationManager.INTERRUPTION_FILTER_ALL).toStatus().doNotDisturb)
        assertTrue(StatusReadings(interruptionFilter = NotificationManager.INTERRUPTION_FILTER_PRIORITY).toStatus().doNotDisturb)
        assertTrue(StatusReadings(interruptionFilter = NotificationManager.INTERRUPTION_FILTER_NONE).toStatus().doNotDisturb)
        assertFalse(StatusReadings(interruptionFilter = NotificationManager.INTERRUPTION_FILTER_UNKNOWN).toStatus().doNotDisturb)
    }

    @Test
    fun aFailedCellularWatcherLeavesTheCellularPartUnset() {
        val read = StatusReadings(
            telephony = true,
            cellLevel = 3,
            serviceState = ServiceState.STATE_IN_SERVICE,
            networkType = TelephonyManager.NETWORK_TYPE_LTE,
            operatorName = "Operator",
        )

        assertFalse(read.withoutCellular().toStatus().hasCellular())
    }

    @Test
    fun aFailedWifiWatcherLeavesTheWifiPartUnset() {
        val status = StatusReadings(wifiLevel = 3).withoutWifi().toStatus()

        assertFalse(status.hasWifi())
    }
}
