package dev.link2linux.android.core.ring

/** The alarm stream's volume, as the guard reads and sets it. */
interface AlarmVolume {
    fun level(): Int

    fun max(): Int

    /** Whether the stream is muted, as by a Do Not Disturb mode that silences alarms: its level and [set] mean nothing then. */
    fun muted(): Boolean

    /** Sets the level; a refusal (as under Do Not Disturb) is not an error. */
    fun set(level: Int)
}

/** Where the level from before ringing waits, so that it outlives the process. */
interface SavedVolume {
    fun load(): Int?

    /** Writes [level] to storage before it returns. */
    fun save(level: Int)

    fun clear()
}

/**
 * Raises the alarm volume for ringing and puts it back. The system keeps a
 * raised volume across a crash, a kill or a reboot, so the level from before
 * is saved first: if the process dies while ringing, the next start calls
 * [repair] and the next real alarm is not at full volume.
 */
class AlarmVolumeGuard(private val volume: AlarmVolume, private val saved: SavedVolume) {
    /**
     * Saves the current level, then sets the maximum. A level saved earlier and
     * not yet restored stays. A muted stream is left alone: nothing would be
     * heard, and the level read from it is not the user's.
     */
    fun raise() {
        if (volume.muted()) return
        if (saved.load() == null) {
            val level = volume.level()
            if (level <= 0) return
            saved.save(level)
        }
        volume.set(volume.max())
    }

    /**
     * Restores the saved level, if any. It is forgotten only once the stream
     * shows it: a refused change (the stream is muted, or the system ignored
     * it) leaves it for the next restore or [repair].
     */
    fun restore() {
        val level = saved.load() ?: return
        if (level <= 0) return saved.clear()
        if (volume.muted()) return
        volume.set(level)
        if (volume.level() == level) saved.clear()
    }

    /**
     * For the start of the process or of the device, when nothing can be
     * ringing: undoes a raise that never got restored. A level that the user
     * changed since (the stream is no longer at the maximum) is left alone.
     */
    fun repair() {
        if (saved.load() == null || volume.muted()) return
        if (volume.level() != volume.max()) return saved.clear()
        restore()
    }
}
