package dev.link2linux.android.core.security

import java.io.File

/**
 * proto/testdata/v1/pairing.txt, the vector the desktop tests check too.
 * The build passes its directory in the `l2l.testdata.dir` system property.
 * Lines are `name = value`; byte strings are hex.
 */
object PairingVectors {

    private val values: Map<String, String> by lazy {
        val dir = checkNotNull(System.getProperty("l2l.testdata.dir")) { "l2l.testdata.dir is not set" }
        File(dir, "v1/pairing.txt").readLines()
            .map(String::trim)
            .filter { it.isNotEmpty() && !it.startsWith("#") }
            .associate { line ->
                val (name, value) = line.split(" = ", limit = 2)
                name to value
            }
    }

    fun text(name: String): String = checkNotNull(values[name]) { "No $name in pairing.txt" }

    fun bytes(name: String): ByteArray = hexBytes(text(name))
}

fun hexBytes(hex: String): ByteArray = ByteArray(hex.length / 2) { i -> hex.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
