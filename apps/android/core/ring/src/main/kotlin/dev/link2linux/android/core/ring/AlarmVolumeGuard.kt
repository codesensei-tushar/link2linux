package dev.link2linux.android.core.ring

/** The alarm stream's volume, as the guard reads and sets it. */
interface AlarmVolume {
    fun level(): Int

    fun max(): Int

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
    /** Saves the current level, then sets the maximum. A level saved earlier and not yet restored stays. */
    fun raise() {
        if (saved.load() == null) saved.save(volume.level())
        volume.set(volume.max())
    }

    /** Restores the saved level, if any, and forgets it. */
    fun restore() {
        val level = saved.load() ?: return
        volume.set(level)
        saved.clear()
    }

    /** For the start of the process, when nothing can be ringing: undoes a raise that never got restored. */
    fun repair() = restore()
}
