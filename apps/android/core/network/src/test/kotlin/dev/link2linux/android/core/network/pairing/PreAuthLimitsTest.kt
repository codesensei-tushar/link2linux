package dev.link2linux.android.core.network.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.net.Inet6Address
import java.net.InetAddress

class PreAuthLimitsTest {

    private fun ip(text: String) = InetAddress.getByName(text)

    @Test
    fun oneHostGetsAtMostItsShareAndOthersStillGetIn() {
        val limits = PreAuthLimits(maxTotal = 4, maxPerSource = 2)
        val first = limits.tryAcquire(ip("192.168.1.9"))
        assertNotNull(first)
        assertNotNull(limits.tryAcquire(ip("192.168.1.9")))
        assertNull(limits.tryAcquire(ip("192.168.1.9")))

        assertNotNull(limits.tryAcquire(ip("192.168.1.20")))
        first!!.release()
        first.release() // twice is harmless
        assertEquals(2, limits.inUse)
        assertNotNull(limits.tryAcquire(ip("192.168.1.9")))
    }

    @Test
    fun theTotalCapHoldsAcrossHosts() {
        val limits = PreAuthLimits(maxTotal = 3, maxPerSource = 2)
        repeat(3) { assertNotNull(limits.tryAcquire(ip("10.0.0.${it + 1}"))) }
        assertNull(limits.tryAcquire(ip("10.0.0.9")))
    }

    @Test
    fun anIpv6HostCountsByItsPrefixAndMappedIpv4ByItsAddress() {
        val limits = PreAuthLimits(maxTotal = 8, maxPerSource = 2)
        assertNotNull(limits.tryAcquire(ip("fd00::1")))
        assertNotNull(limits.tryAcquire(ip("fd00::2")))
        assertNull(limits.tryAcquire(ip("fd00::ffff:3")))
        assertNotNull(limits.tryAcquire(ip("fd00:0:0:1::1")))

        val mapped = Inet6Address.getByAddress(null, ByteArray(10) + byteArrayOf(-1, -1, 10, 0, 0, 7), -1)
        assertEquals("10.0.0.7", PreAuthLimits.source(mapped))
        assertNotEquals(PreAuthLimits.source(ip("10.0.0.7")), PreAuthLimits.source(ip("10.0.0.8")))
    }
}
