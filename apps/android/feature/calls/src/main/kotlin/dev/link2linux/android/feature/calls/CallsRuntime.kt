package dev.link2linux.android.feature.calls

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import dagger.hilt.android.qualifiers.ApplicationContext
import dev.link2linux.android.core.model.Feature
import dev.link2linux.protocol.v1.FeatureAvailability
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject
import javax.inject.Singleton

/** What a phone lacks for calls on the computer, if anything. */
enum class CallsSupport { SUPPORTED, NO_TELEPHONY, NO_BLUETOOTH }

/**
 * The process-wide calls feature: the setting, what the phone supports,
 * and the prompt that helps the user pair with a computer over Bluetooth.
 * Calls themselves go over Bluetooth hands-free, which Android runs; the
 * session reaches this feature through [availability] and [promptPairing]
 * (see CallsSessionHooks).
 */
@Singleton
class CallsRuntime @Inject internal constructor(@ApplicationContext context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("calls", Context.MODE_PRIVATE)
    private val pairing = PairingNotifier(app)

    val support: CallsSupport = supportOf(app.packageManager)

    private val _enabled = MutableStateFlow(prefs.getBoolean(ENABLED, Feature.CALLS.isOnByDefault))

    /** Whether the user has the feature on: on until turned off. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    private val _availability = MutableStateFlow(availabilityOf(_enabled.value))

    /** The feature's state on this phone, as its `FeatureStatus` reports it. */
    val availability: StateFlow<FeatureAvailability> = _availability.asStateFlow()

    fun setEnabled(on: Boolean) {
        prefs.edit { putBoolean(ENABLED, on) }
        _enabled.value = on
        _availability.value = availabilityOf(on)
        if (!on) pairing.cancel()
    }

    /**
     * Asks the user to pair with the computer adapter [name] at [address], for the computer's
     * pairing window. Returns false when the phone cannot show the prompt.
     */
    fun promptPairing(name: String, address: String) = pairing.show(name, address)

    /** Whether tapping the pairing prompt starts pairing at once, which needs the nearby devices permission. */
    fun canPairDirectly(): Boolean = canPairDirectly(app)

    private fun availabilityOf(enabled: Boolean): FeatureAvailability = when {
        support != CallsSupport.SUPPORTED -> FeatureAvailability.FEATURE_AVAILABILITY_UNAVAILABLE
        enabled -> FeatureAvailability.FEATURE_AVAILABILITY_READY
        else -> FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
    }

    internal companion object {
        private const val ENABLED = "enabled"

        /** The permission that lets the app start pairing itself; null before Android 12, which has none to ask for. */
        val PAIR_PERMISSION: String? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) Manifest.permission.BLUETOOTH_CONNECT else null

        fun canPairDirectly(context: Context): Boolean = PAIR_PERMISSION?.let {
            ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
        } == true

        private fun supportOf(packages: PackageManager): CallsSupport {
            // A tablet with a data-only SIM has telephony but cannot call.
            val calling = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                PackageManager.FEATURE_TELEPHONY_CALLING
            } else {
                PackageManager.FEATURE_TELEPHONY
            }
            return when {
                !packages.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH) -> CallsSupport.NO_BLUETOOTH
                !packages.hasSystemFeature(calling) -> CallsSupport.NO_TELEPHONY
                else -> CallsSupport.SUPPORTED
            }
        }
    }
}
