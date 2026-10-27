package dev.link2linux.android.feature.calls

import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.provider.Settings
import android.util.Log

/**
 * Opened by the pairing prompt, and shows nothing itself. Where the user
 * allowed the app to (Android 12 and later, nearby devices), it starts
 * pairing with the computer's address; otherwise, or when that fails, it
 * opens Bluetooth settings, which list the computer as a new device while
 * it accepts pairing. Either way Android then shows the code to compare.
 */
class PairComputerActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val address = intent.getStringExtra(EXTRA_ADDRESS)?.takeIf(BluetoothAdapter::checkBluetoothAddress)
        if (address == null || !pairDirectly(address)) openBluetoothSettings()
        finish()
    }

    // canPairDirectly checks the permission that every call below needs.
    @SuppressLint("MissingPermission")
    private fun pairDirectly(address: String): Boolean {
        if (!CallsRuntime.canPairDirectly(this)) return false
        val adapter = getSystemService(BluetoothManager::class.java)?.adapter ?: return false
        return try {
            adapter.isEnabled && adapter.getRemoteDevice(address).createBond()
        } catch (e: SecurityException) {
            Log.w(TAG, "Could not start pairing: ${e.javaClass.simpleName}")
            false
        }
    }

    private fun openBluetoothSettings() {
        try {
            startActivity(Intent(Settings.ACTION_BLUETOOTH_SETTINGS))
        } catch (e: ActivityNotFoundException) {
            Log.w(TAG, "No Bluetooth settings: ${e.javaClass.simpleName}")
        }
    }

    internal companion object {
        private const val TAG = "link2linux"
        private const val EXTRA_ADDRESS = "dev.link2linux.android.calls.ADDRESS"

        fun intent(context: Context, address: String): Intent =
            Intent(context, PairComputerActivity::class.java).putExtra(EXTRA_ADDRESS, address)
    }
}
