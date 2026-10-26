package dev.link2linux.android.core.discovery

import org.junit.Assume.assumeTrue
import java.io.File

/** `proto/testdata/v1/pairing.txt`, parsed; tests skip when the checkout lacks it. */
class PairingVector private constructor(private val values: Map<String, String>) {

    operator fun get(name: String): String = values.getValue(name)

    fun bytes(name: String): ByteArray = get(name).chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    companion object {
        fun load(): PairingVector {
            val file = System.getProperty("l2l.testdata.dir")?.let { File(it, "v1/pairing.txt") }
            assumeTrue("pairing vector not present", file?.isFile == true)
            val values = file!!.readLines()
                .filter { it.isNotBlank() && !it.startsWith("#") }
                .associate { line -> line.substringBefore(" = ") to line.substringAfter(" = ") }
            return PairingVector(values)
        }
    }
}
