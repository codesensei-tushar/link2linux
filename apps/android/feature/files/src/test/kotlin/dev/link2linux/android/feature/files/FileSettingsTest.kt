package dev.link2linux.android.feature.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FileSettingsTest {
    private class MemoryStore : FileSettingsStore {
        val booleans = mutableMapOf<String, Boolean>()

        override fun getBoolean(key: String, default: Boolean) = booleans[key] ?: default

        override fun putBoolean(key: String, value: Boolean) {
            booleans[key] = value
        }
    }

    @Test
    fun `files are off until the user turns them on`() {
        assertFalse(FileSettings(MemoryStore()).enabled.value)
    }

    @Test
    fun `the choice is stored, read back by a new instance, and reported`() {
        val store = MemoryStore()
        var changes = 0
        val settings = FileSettings(store) { changes++ }
        settings.setEnabled(true)
        assertTrue(settings.enabled.value)
        assertEquals(1, changes)
        assertTrue(FileSettings(store).enabled.value)
        settings.setEnabled(false)
        assertFalse(FileSettings(store).enabled.value)
        assertEquals(2, changes)
    }
}
