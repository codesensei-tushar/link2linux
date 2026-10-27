package dev.link2linux.android.core.ring

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AlarmVolumeGuardTest {
    private class FakeVolume(var level: Int, private val max: Int = 15) : AlarmVolume {
        var muted = false

        override fun level() = if (muted) 0 else level

        override fun max() = max

        override fun muted() = muted

        /** Like the system under Do Not Disturb: a muted stream ignores the change. */
        override fun set(level: Int) {
            if (!muted) this.level = level
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

    @Test
    fun `a restore refused under Do Not Disturb keeps the level for later`() {
        val volume = FakeVolume(3)
        val saved = FakeSaved()
        val guard = AlarmVolumeGuard(volume, saved)
        guard.raise()

        volume.muted = true
        guard.restore()
        assertEquals(3, saved.value)

        volume.muted = false
        guard.repair()
        assertEquals(3, volume.level)
        assertNull(saved.value)
    }

    @Test
    fun `a restore that does not stick keeps the level`() {
        val volume = object : AlarmVolume {
            override fun level() = 15

            override fun max() = 15

            override fun muted() = false

            override fun set(level: Int) = Unit
        }
        val saved = FakeSaved()
        saved.value = 3

        AlarmVolumeGuard(volume, saved).restore()

        assertEquals(3, saved.value)
    }

    @Test
    fun `a muted stream is not raised and nothing is saved`() {
        val volume = FakeVolume(3)
        val saved = FakeSaved()
        volume.muted = true

        AlarmVolumeGuard(volume, saved).raise()

        assertNull(saved.value)
        assertEquals(3, volume.level)
    }

    @Test
    fun `a zero reading is never saved`() {
        val volume = FakeVolume(0)
        val saved = FakeSaved()

        AlarmVolumeGuard(volume, saved).raise()

        assertNull(saved.value)
        assertEquals(0, volume.level)
    }

    @Test
    fun `a saved zero is dropped without touching the volume`() {
        val volume = FakeVolume(15)
        val saved = FakeSaved()
        saved.value = 0

        AlarmVolumeGuard(volume, saved).restore()

        assertNull(saved.value)
        assertEquals(15, volume.level)
    }

    @Test
    fun `repair leaves a level the user chose after the raise`() {
        val volume = FakeVolume(3)
        val saved = FakeSaved()
        AlarmVolumeGuard(volume, saved).raise()
        volume.level = 8

        AlarmVolumeGuard(volume, saved).repair()

        assertEquals(8, volume.level)
        assertNull(saved.value)
    }
}
