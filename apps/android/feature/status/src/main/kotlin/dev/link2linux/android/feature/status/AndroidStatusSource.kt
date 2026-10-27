package dev.link2linux.android.feature.status

import android.app.NotificationManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.media.AudioManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.telephony.AccessNetworkConstants
import android.telephony.NetworkRegistrationInfo
import android.telephony.PhoneStateListener
import android.telephony.ServiceState
import android.telephony.SignalStrength
import android.telephony.TelephonyCallback
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat
import dev.link2linux.android.core.status.StatusSource
import dev.link2linux.protocol.v1.PhoneStatus
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update

/**
 * The phone's battery, signal and sound mode, read with no permission.
 *
 * Collecting [statuses] reads what Android tells at once, then registers
 * the listeners, all on the main thread, and stops them when cancelled.
 * Two parts need permissions the app does not ask for, so they stay unset
 * on some versions: the cellular generation before Android 11 (from 12 on
 * it comes from the status bar's display info where Android allows it,
 * else from the data registration), and the Wi-Fi level before Android 10.
 */
internal class AndroidStatusSource(context: Context) : StatusSource {
    private val app = context.applicationContext
    private val audio = app.getSystemService(AudioManager::class.java)
    private val notifications = app.getSystemService(NotificationManager::class.java)
    private val power = app.getSystemService(PowerManager::class.java)
    private val connectivity = app.getSystemService(ConnectivityManager::class.java)
    private val wifi = app.getSystemService(WifiManager::class.java)
    private val telephony = app.getSystemService(TelephonyManager::class.java)
        ?.takeIf { app.packageManager.hasSystemFeature(PackageManager.FEATURE_TELEPHONY) }

    override fun statuses(): Flow<PhoneStatus> = flow {
        val readings = MutableStateFlow(StatusReadings(telephony = telephony != null))
        val stops = listOf(
            watchBroadcasts(readings),
            watchTelephony(readings),
            watchWifi(readings),
        )
        try {
            emitAll(readings.map { it.toStatus() })
        } finally {
            stops.forEach { it() }
        }
    }.flowOn(Dispatchers.Main)

    /** Battery, battery saver, ringer mode and Do Not Disturb, each read at once and then on its broadcast. */
    private fun watchBroadcasts(readings: MutableStateFlow<StatusReadings>): () -> Unit {
        fun read(battery: Intent?) = readings.update {
            it.copy(
                batteryLevel = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: it.batteryLevel,
                batteryScale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: it.batteryScale,
                plugged = battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: it.plugged,
                powerSaver = power?.isPowerSaveMode == true,
                ringerMode = audio?.ringerMode,
                interruptionFilter = notifications?.currentInterruptionFilter,
            )
        }

        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                read(intent.takeIf { it.action == Intent.ACTION_BATTERY_CHANGED })
            }
        }
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_BATTERY_CHANGED)
            addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED)
            addAction(AudioManager.RINGER_MODE_CHANGED_ACTION)
            addAction(NotificationManager.ACTION_INTERRUPTION_FILTER_CHANGED)
        }
        // System broadcasts only, which reach a receiver that is not exported. The battery's is
        // sticky: registering returns the latest one.
        read(ContextCompat.registerReceiver(app, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED))
        return { app.unregisterReceiver(receiver) }
    }

    /** The signal bars, service and generation, and the operator's name. */
    private fun watchTelephony(readings: MutableStateFlow<StatusReadings>): () -> Unit {
        val telephony = telephony ?: return {}
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            telephony.signalStrength?.let { strength -> readings.update { it.copy(cellLevel = strength.level) } }
        }
        readings.update { it.copy(operatorName = operatorName(telephony)) }

        val signal = { strength: SignalStrength -> readings.update { it.copy(cellLevel = strength.level) } }
        val service = { state: ServiceState ->
            readings.update {
                it.copy(
                    serviceState = state.state,
                    operatorName = operatorName(telephony),
                    // The status bar's own type, where Android tells it, is the better one.
                    networkType = if (it.overrideNetworkType != null) it.networkType else dataNetworkType(state),
                )
            }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val executor = ContextCompat.getMainExecutor(app)
            val display = { info: TelephonyDisplayInfo ->
                readings.update { it.copy(networkType = info.networkType, overrideNetworkType = info.overrideNetworkType) }
            }
            // Display info may need READ_PHONE_STATE, which the app does not ask for; then only the rest.
            val callback = try {
                Callbacks.WithDisplay(signal, service, display).also { telephony.registerTelephonyCallback(executor, it) }
            } catch (e: SecurityException) {
                Log.d(TAG, "No display info: ${e.javaClass.simpleName}")
                Callbacks(signal, service).also { telephony.registerTelephonyCallback(executor, it) }
            }
            return { telephony.unregisterTelephonyCallback(callback) }
        }
        @Suppress("DEPRECATION")
        val listener = object : PhoneStateListener() {
            @Deprecated("Deprecated in Java")
            override fun onSignalStrengthsChanged(signalStrength: SignalStrength) = signal(signalStrength)

            @Deprecated("Deprecated in Java")
            override fun onServiceStateChanged(serviceState: ServiceState) = service(serviceState)
        }
        @Suppress("DEPRECATION")
        telephony.listen(listener, PhoneStateListener.LISTEN_SIGNAL_STRENGTHS or PhoneStateListener.LISTEN_SERVICE_STATE)
        @Suppress("DEPRECATION")
        return { telephony.listen(listener, PhoneStateListener.LISTEN_NONE) }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private open class Callbacks(
        private val signal: (SignalStrength) -> Unit,
        private val service: (ServiceState) -> Unit,
    ) : TelephonyCallback(),
        TelephonyCallback.SignalStrengthsListener,
        TelephonyCallback.ServiceStateListener {
        override fun onSignalStrengthsChanged(signalStrength: SignalStrength) = signal(signalStrength)

        override fun onServiceStateChanged(serviceState: ServiceState) = service(serviceState)

        class WithDisplay(
            signal: (SignalStrength) -> Unit,
            service: (ServiceState) -> Unit,
            private val display: (TelephonyDisplayInfo) -> Unit,
        ) : Callbacks(signal, service),
            TelephonyCallback.DisplayInfoListener {
            override fun onDisplayInfoChanged(telephonyDisplayInfo: TelephonyDisplayInfo) = display(telephonyDisplayInfo)
        }
    }

    /** The network type of the cellular data registration, not Wi-Fi calling; it tells no location, so it is not withheld. */
    private fun dataNetworkType(state: ServiceState): Int? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return null
        return state.networkRegistrationInfoList
            .firstOrNull {
                (it.domain and NetworkRegistrationInfo.DOMAIN_PS) != 0 &&
                    it.transportType == AccessNetworkConstants.TRANSPORT_TYPE_WWAN &&
                    it.isRegistered
            }
            ?.accessNetworkTechnology
    }

    // The registered network's name, else the SIM's, as the status bar falls back.
    private fun operatorName(telephony: TelephonyManager): String =
        telephony.networkOperatorName?.takeIf { it.isNotBlank() } ?: telephony.simOperatorName.orEmpty()

    /** The Wi-Fi networks the phone is on, and the best one's level. */
    private fun watchWifi(readings: MutableStateFlow<StatusReadings>): () -> Unit {
        val connectivity = connectivity ?: return {}
        val levels = HashMap<Network, Int?>() // on the main thread only
        fun publish() = readings.update { it.copy(wifiLevel = levels.values.filterNotNull().maxOrNull()) }

        connectivity.activeNetwork?.let { network ->
            connectivity.getNetworkCapabilities(network)?.takeIf { it.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) }?.let {
                levels[network] = levelOf(it)
                publish()
            }
        }
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                levels[network] = levelOf(capabilities)
                publish()
            }

            override fun onLost(network: Network) {
                levels -= network
                publish()
            }
        }
        // Every Wi-Fi network, also one without internet such as the computer's hotspot.
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        connectivity.registerNetworkCallback(request, callback, Handler(Looper.getMainLooper()))
        return { connectivity.unregisterNetworkCallback(callback) }
    }

    /** The level of a Wi-Fi network, 0 to 4, as the status bar shows it; null where Android does not tell. */
    private fun levelOf(capabilities: NetworkCapabilities): Int? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
        val rssi = capabilities.signalStrength
        if (rssi == NetworkCapabilities.SIGNAL_STRENGTH_UNSPECIFIED) return null
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && wifi != null) {
            StatusReadings.wifiLevel(wifi.calculateSignalLevel(rssi), wifi.maxSignalLevel)
        } else {
            @Suppress("DEPRECATION")
            StatusReadings.wifiLevel(WifiManager.calculateSignalLevel(rssi, LEGACY_LEVELS), LEGACY_LEVELS - 1)
        }
    }

    private companion object {
        const val TAG = "link2linux"

        const val LEGACY_LEVELS = 5
    }
}
