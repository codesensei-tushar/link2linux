package dev.link2linux.android.feature.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class KeptGrantsTest {
    private val taken = mutableListOf<String>()
    private val released = mutableListOf<String>()
    private var refuse = false
    private val grants = KeptGrants<String>(
        take = { if (refuse) false else taken.add(it) },
        release = { released += it },
    )

    @Test
    fun `a document sent twice keeps its grant until both sends ended`() {
        assertTrue(grants.keep("doc"))
        assertTrue(grants.keep("doc"))
        assertEquals(listOf("doc"), taken)
        grants.letGo("doc")
        assertTrue(released.isEmpty())
        grants.letGo("doc")
        assertEquals(listOf("doc"), released)
        // Let go once more, it is not released twice.
        grants.letGo("doc")
        assertEquals(listOf("doc"), released)
    }

    @Test
    fun `a grant that cannot be kept is not counted`() {
        refuse = true
        assertFalse(grants.keep("doc"))
        grants.letGo("doc")
        assertTrue(released.isEmpty())
    }

    @Test
    fun `grants of an earlier run are let go, the ones in use stay`() {
        grants.keep("now")
        grants.letGoOfStale(listOf("old", "now", "older"))
        assertEquals(listOf("old", "older"), released)
    }
}
