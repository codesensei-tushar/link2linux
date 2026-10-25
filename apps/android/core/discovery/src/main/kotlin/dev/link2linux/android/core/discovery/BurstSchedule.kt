package dev.link2linux.android.core.discovery

/** The beacon bursts of docs/protocol.md ("Beacons"), as the times of their beacons. */
object BurstSchedule {

    /** The partial wake lock covers a burst only for this long. */
    const val WAKE_LOCK_MILLIS: Long = 60_000

    /** While waiting for the desktop after a QR scan: one per second, at most 2 minutes. */
    val qrWait: List<Long> = (0 until 120).map { it * 1_000L }

    /**
     * With a pairing but no session: one per second for 10 seconds, one every
     * 5 seconds up to 1 minute, then one every 30 seconds up to 10 minutes.
     */
    val pairedNoSession: List<Long> =
        (0 until 10).map { it * 1_000L } +
            (2..12).map { it * 5_000L } +
            (3..20).map { it * 30_000L }

    /** Whether the beacon at [offsetMillis] is sent with the device kept awake. */
    fun holdsWakeLock(offsetMillis: Long): Boolean = offsetMillis < WAKE_LOCK_MILLIS
}
