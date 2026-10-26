package dev.link2linux.android.core.discovery

import kotlinx.coroutines.flow.StateFlow
import java.net.InetSocketAddress

/** Registers the phone's mDNS service. One registration at a time. */
interface ServiceAdvertiser {

    /** The instance name Android registered the service under, or null while none is registered. */
    val registeredName: StateFlow<String?>

    /**
     * Replaces the current registration with one for [announcement] and
     * [port]. Registering again makes Android send fresh announcements, which
     * is what a beacon burst relies on.
     */
    suspend fun register(announcement: Announcement, port: Int)

    suspend fun unregister()
}

/** Sends a beacon on the phone's networks. */
interface BeaconSender {

    /**
     * Sends [datagram] to the broadcast addresses of each of [networks] and,
     * by unicast, to those of [unicast] that are on-link for one of them.
     * Failures are swallowed: beacons are best effort.
     */
    suspend fun send(networks: List<LocalNetwork>, datagram: ByteArray, unicast: List<InetSocketAddress>)
}

/** A lock held for as long as something lasts, such as a pairing attempt. */
interface HeldLock {
    fun acquire()
    fun release()
}

/** A lock that is held for a while and released on its own. */
interface TimedLock {

    /** Holds the lock for at most [millis] from now. */
    fun hold(millis: Long)

    fun release()
}
