package dev.link2linux.android.core.network.pairing

import java.net.InetAddress
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Caps on the connections that have not yet finished the TLS handshake and
 * the Hello exchange: at most [maxTotal] at a time, and at most
 * [maxPerSource] from one [source]. A host that opens many connections and
 * never finishes them then holds a few slots, not every thread of the app,
 * and a desktop on another address still gets through.
 */
internal class PreAuthLimits(private val maxTotal: Int, private val maxPerSource: Int) {

    private val perSource = HashMap<String, Int>()
    private var total = 0

    /** A slot one connection holds until [release]. Releasing twice is harmless. */
    inner class Slot internal constructor(private val source: String) {
        private val released = AtomicBoolean(false)

        fun release() {
            if (released.compareAndSet(false, true)) free(source)
        }
    }

    /** Takes a slot for a connection from [address], or returns null when a cap is reached. */
    @Synchronized
    fun tryAcquire(address: InetAddress): Slot? {
        val source = source(address)
        val fromSource = perSource[source] ?: 0
        if (total >= maxTotal || fromSource >= maxPerSource) return null
        total++
        perSource[source] = fromSource + 1
        return Slot(source)
    }

    /** How many slots are taken now. */
    @get:Synchronized
    val inUse: Int get() = total

    @Synchronized
    private fun free(source: String) {
        total--
        val left = (perSource[source] ?: 1) - 1
        if (left <= 0) perSource.remove(source) else perSource[source] = left
    }

    companion object {
        /**
         * Who a connection counts against: its IPv4 address (also when it
         * arrives IPv4-mapped), or the /64 of its IPv6 address, as one host
         * can use any address in its prefix.
         */
        fun source(address: InetAddress): String {
            val bytes = address.address
            val mapped = bytes.size == 16 && (0 until 10).all { bytes[it] == 0.toByte() } &&
                bytes[10] == (-1).toByte() && bytes[11] == (-1).toByte()
            return when {
                bytes.size == 4 -> ipv4(bytes)
                mapped -> ipv4(bytes.copyOfRange(12, 16))
                else -> bytes.copyOf(8).joinToString(":", postfix = "::/64") { "%02x".format(it) }
            }
        }

        private fun ipv4(bytes: ByteArray) = bytes.joinToString(".") { (it.toInt() and 0xff).toString() }
    }
}
