package dev.link2linux.android.core.model

import org.junit.Assert.assertEquals
import org.junit.Test

class SasCodeTest {

    @Test
    fun sasCodeShowsTwoGroupsWithLeadingZeros() {
        assertEquals("979 999", SasCode(979_999).formatted)
        assertEquals("004 217", SasCode(4_217).formatted)
        assertEquals("000 000", SasCode(0).formatted)
    }

    @Test(expected = IllegalArgumentException::class)
    fun sasCodeHasSixDigits() {
        SasCode(1_000_000)
    }
}
