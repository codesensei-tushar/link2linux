package dev.link2linux.android.core.ring

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AlarmVolumeGuardTest {
    private class FakeVolume(var level: Int, private val max: Int = 15) : AlarmVolume {
        override fun level() = level

        override fun max() = max

        override fun set(level: Int) {
            this.level = level
        }
    }

    /** Stands for the preferences file: it is kept when the guard and the player are gone. */
    private class FakeSaved : SavedVolume {
        var value: Int? = null

        override fun load() = value

        override fun save(level: Int) {
            value = level
        }

        override fun clear() {
            value = null
        }
    }

    @Test
    fun `raise saves the level before the maximum is set, and restore puts it back`() {
        val volume = FakeVolume(3)
        val saved = FakeSaved()
        val guard = AlarmVolumeGuard(volume, saved)

        guard.raise()
        assertEquals(15, volume.level)
        assertEquals(3, saved.value)

        guard.restore()
        assertEquals(3, volume.level)
        assertNull(saved.value)
    }

    @Test
    fun `a new guard after a death while ringing restores the level on start-up`() {
        val volume = FakeVolume(3)
        val saved = FakeSaved()
        AlarmVolumeGuard(volume, saved).raise()

        AlarmVolumeGuard(volume, saved).repair()

        assertEquals(3, volume.level)
        assertNull(saved.value)
    }

    @Test
    fun `repair leaves the volume alone when nothing was raised`() {
        val volume = FakeVolume(7)

        AlarmVolumeGuard(volume, FakeSaved()).repair()

        assertEquals(7, volume.level)
    }

    @Test
    fun `raising again before a restore keeps the first level`() {
        val volume = FakeVolume(3)
        val saved = FakeSaved()
        val guard = AlarmVolumeGuard(volume, saved)

        guard.raise()
        guard.raise()
        guard.restore()

        assertEquals(3, volume.level)
    }

    @Test
    fun `restore without a raise changes nothing`() {
        val volume = FakeVolume(9)

        AlarmVolumeGuard(volume, FakeSaved()).restore()

        assertEquals(9, volume.level)
    }
}
