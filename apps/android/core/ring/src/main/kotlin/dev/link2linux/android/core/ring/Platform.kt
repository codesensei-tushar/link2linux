package dev.link2linux.android.core.ring

import dev.link2linux.protocol.v1.RingEndReason

/*
 * The seams between the ring engine, the phone and the session. The
 * platform plays, vibrates and shows the notification; the session attaches
 * a [RingPeer] for each desktop where the feature is active and passes the
 * desktop's requests to the engine.
 */

/** The ringtone, as the engine plays it. */
interface RingPlayer {
    /**
     * Starts the ringtone at full volume, also when the ringer is set to
     * vibrate or silent. Returns false when the phone cannot ring now, for
     * example during a call. Once started, [onInterrupted] runs, on any
     * thread, when the phone cannot go on, for example because a call
     * started or another app took the audio.
     */
    fun start(onInterrupted: () -> Unit): Boolean

    /** Stops the ringtone and restores the volume it raised. */
    fun stop()
}

/** The vibration that goes with the ringtone. */
interface RingVibrator {
    fun start()

    fun stop()
}

/** The notification that shows the phone rings, with a button that stops it. */
interface RingNotifier {
    /** Shows it, naming [desktopName], the desktop that asked last; replaces one already shown. */
    fun show(desktopName: String)

    fun cancel()
}

/** A connected desktop where the feature is active. */
interface RingPeer {
    /** Names the desktop, the same in each of its sessions. */
    val desktopId: String

    /** Sends `RingEnded` with [reason]. Throws when the connection fails. */
    suspend fun ended(reason: RingEndReason)
}

/** The phone's answer to a `RingRequest`. */
enum class RingAnswer {
    /** It rings, for a full period from now. */
    RINGING,

    /** The user turned the feature off. */
    DISABLED,

    /** It cannot ring now, for example during a call. */
    UNAVAILABLE,
}
