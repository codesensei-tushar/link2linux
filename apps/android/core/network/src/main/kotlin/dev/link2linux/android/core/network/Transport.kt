package dev.link2linux.android.core.network

import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream

/**
 * An established, already secured byte stream to a peer. On the phone it is
 * a [dev.link2linux.android.core.network.tls.TlsTransport] accepted by the
 * TLS server.
 */
interface Transport : Closeable {
    val input: InputStream
    val output: OutputStream
}

/** Derives connection-bound values from a TLS connection (RFC 8446 section 7.5). */
fun interface KeyingMaterialExporter {

    /**
     * The exporter's output, or null if this connection cannot export, for
     * example because it is closed. A caller that needs the value to pair
     * must then refuse to pair.
     */
    fun exportKeyingMaterial(label: String, context: ByteArray, length: Int): ByteArray?
}
