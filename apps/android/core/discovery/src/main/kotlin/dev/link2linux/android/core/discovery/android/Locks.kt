package dev.link2linux.android.core.discovery.android

import android.content.Context
import android.net.wifi.WifiManager
import android.os.PowerManager
import dev.link2linux.android.core.discovery.HeldLock
import dev.link2linux.android.core.discovery.TimedLock

/**
 * A partial wake lock that times out on its own, so a forgotten release
 * costs seconds, not battery. Beacons use it during the first minute of a
 * burst only.
 */
class BeaconWakeLock(context: Context) : TimedLock {

    private val lock = context.applicationContext.getSystemService(PowerManager::class.java)
        .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "link2linux:beacons")
        .apply { setReferenceCounted(false) }

    override fun hold(millis: Long) = lock.acquire(millis)

    override fun release() {
        if (lock.isHeld) lock.release()
    }
}

/**
 * The Wi-Fi multicast lock, held only while pairing. Without it the Wi-Fi
 * firmware drops inbound multicast, and the phone would not hear mDNS
 * traffic while the app is visible and pairing.
 */
class PairingMulticastLock(context: Context) : HeldLock {

    private val lock = context.applicationContext.getSystemService(WifiManager::class.java)
        .createMulticastLock("link2linux:pairing")
        .apply { setReferenceCounted(false) }

    override fun acquire() {
        if (!lock.isHeld) lock.acquire()
    }

    override fun release() {
        if (lock.isHeld) lock.release()
    }
}
