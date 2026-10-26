package dev.link2linux.android.feature.notifications

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationSettingsTest {
    private class MemoryStore : SettingsStore {
        val booleans = mutableMapOf<String, Boolean>()
        val strings = mutableMapOf<String, Set<String>>()

        override fun getBoolean(key: String, default: Boolean) = booleans[key] ?: default

        override fun putBoolean(key: String, value: Boolean) {
            booleans[key] = value
        }

        override fun getStrings(key: String) = strings[key].orEmpty()

        override fun putStrings(key: String, value: Set<String>) {
            strings[key] = value
        }
    }

    @Test
    fun `the feature is off, dismiss sync and code hiding are on and nothing is filtered by default`() {
        val s = NotificationSettings(MemoryStore()).state.value
        assertFalse(s.enabled)
        assertTrue(s.dismissSync)
        assertTrue(s.hideOneTimeCodes)
        assertTrue(s.excluded.isEmpty())
        assertTrue(s.contentHidden.isEmpty())
    }

    @Test
    fun `choices are stored and read back by a new instance`() {
        val store = MemoryStore()
        val a = NotificationSettings(store)
        a.setEnabled(true)
        a.setDismissSync(false)
        a.setHideOneTimeCodes(false)
        a.setShared("org.chat", false)
        a.setContentHidden("org.mail", true)
        a.rememberApp("org.chat", "Chat")
        val b = NotificationSettings(store).state.value
        assertTrue(b.enabled)
        assertFalse(b.dismissSync)
        assertFalse(b.hideOneTimeCodes)
        assertEquals(setOf("org.chat"), b.excluded)
        assertEquals(setOf("org.mail"), b.contentHidden)
        assertEquals(mapOf("org.chat" to "Chat"), b.knownApps)
    }

    @Test
    fun `sharing an app again and showing its text again undo the choice`() {
        val s = NotificationSettings(MemoryStore())
        s.setShared("p", false)
        s.setShared("p", true)
        s.setContentHidden("p", true)
        s.setContentHidden("p", false)
        assertTrue(s.state.value.excluded.isEmpty())
        assertTrue(s.state.value.contentHidden.isEmpty())
    }

    @Test
    fun `changes notify, remembering an app does not`() {
        var calls = 0
        val s = NotificationSettings(MemoryStore()) { calls++ }
        s.setEnabled(true)
        s.setShared("p", false)
        s.setHideOneTimeCodes(false)
        s.rememberApp("p", "P")
        assertEquals(3, calls)
    }

    @Test
    fun `an app name may contain an equals sign`() {
        val store = MemoryStore()
        NotificationSettings(store).rememberApp("p", "A=B")
        assertEquals("A=B", NotificationSettings(store).state.value.knownApps["p"])
    }
}
