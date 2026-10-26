package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.QrPairingOffer
import dev.link2linux.android.core.model.withoutHiddenCharacters
import java.net.InetAddress
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.util.Base64

/** The outcome of reading a scanned QR code. */
sealed interface QrParseResult {

    /** A valid offer. */
    data class Parsed(val offer: QrPairingOffer) : QrParseResult

    /** The code is a link2linux code of a payload version this app does not know: "update the app". */
    data object UpdateRequired : QrParseResult

    /** The text is not a valid pairing code. */
    data class Invalid(val reason: Reason) : QrParseResult

    enum class Reason {
        /** Too long, not ASCII, or not `link2linux://pair?...`. */
        NOT_A_PAIRING_CODE,

        /** A parameter appears twice, or is not `key=value`. */
        MALFORMED,

        MISSING_OR_BAD_FINGERPRINT,
        MISSING_OR_BAD_SECRET,
        MISSING_OR_BAD_NAME,
        BAD_ADDRESSES,
        BAD_BLUETOOTH_ADDRESS,
    }
}

/**
 * Reads the URI in a desktop's QR code (docs/protocol.md, "QR code payload").
 * The parser is strict: everything it accepts has exactly one reading, and
 * it never resolves a name, so reading a code cannot make a network request.
 */
object QrOfferParser {

    const val MAX_LENGTH: Int = 1024
    const val MAX_NAME_BYTES: Int = 64
    const val MAX_ADDRESSES: Int = 4

    private const val PREFIX = "link2linux://pair?"
    private const val SUPPORTED_VERSION = "1"

    fun parse(text: String): QrParseResult {
        if (text.length > MAX_LENGTH || !text.startsWith(PREFIX) || !text.all { it.code in 0x21..0x7e }) {
            return invalid(QrParseResult.Reason.NOT_A_PAIRING_CODE)
        }
        if ('#' in text) return invalid(QrParseResult.Reason.NOT_A_PAIRING_CODE)

        val pairs = text.substring(PREFIX.length).split('&').map { part ->
            val eq = part.indexOf('=')
            if (eq <= 0) return invalid(QrParseResult.Reason.MALFORMED)
            part.substring(0, eq) to part.substring(eq + 1)
        }

        // The version decides how to read everything else, so it goes first.
        val versions = pairs.filter { it.first == "v" }
        if (versions.size > 1) return invalid(QrParseResult.Reason.MALFORMED)
        val version = versions.singleOrNull()?.second ?: return invalid(QrParseResult.Reason.MALFORMED)
        if (version != SUPPORTED_VERSION) return QrParseResult.UpdateRequired

        val params = HashMap<String, String>()
        for ((key, value) in pairs) {
            if (params.put(key, value) != null) return invalid(QrParseResult.Reason.MALFORMED)
        }

        val fingerprint = params["f"]?.let(::fingerprint)
            ?: return invalid(QrParseResult.Reason.MISSING_OR_BAD_FINGERPRINT)
        val secret = params["s"]?.let(::secret)
            ?: return invalid(QrParseResult.Reason.MISSING_OR_BAD_SECRET)
        val name = params["n"]?.let(::name)
            ?: return invalid(QrParseResult.Reason.MISSING_OR_BAD_NAME)
        val addresses = params["a"]?.let { addresses(it) ?: return invalid(QrParseResult.Reason.BAD_ADDRESSES) }
            ?: emptyList()
        val bluetooth = params["b"]?.let { bluetoothAddress(it) ?: return invalid(QrParseResult.Reason.BAD_BLUETOOTH_ADDRESS) }

        return QrParseResult.Parsed(QrPairingOffer(fingerprint, secret, name, addresses, bluetooth))
    }

    private fun invalid(reason: QrParseResult.Reason) = QrParseResult.Invalid(reason)

    /** Exactly [length] base64url characters that are the canonical encoding of their bytes. */
    private fun canonicalBase64Url(text: String, length: Int, size: Int): ByteArray? {
        if (text.length != length) return null
        val bytes = try {
            Base64.getUrlDecoder().decode(text)
        } catch (_: IllegalArgumentException) {
            return null
        }
        if (bytes.size != size || bytes.toBase64Url() != text) return null
        return bytes
    }

    private fun fingerprint(text: String): KeyFingerprint? =
        canonicalBase64Url(text, 43, KeyFingerprint.SIZE)?.let(KeyFingerprint::of)

    private fun secret(text: String): PairingSecret? =
        canonicalBase64Url(text, 22, PairingSecret.SIZE)?.let(PairingSecret::of)

    private fun name(encoded: String): String? {
        val bytes = percentDecode(encoded) ?: return null
        if (bytes.isEmpty() || bytes.size > MAX_NAME_BYTES) return null
        val decoder = Charsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
        val name = try {
            decoder.decode(ByteBuffer.wrap(bytes)).toString()
        } catch (_: CharacterCodingException) {
            return null
        }
        // A desktop never sends controls. Format characters can belong in a
        // name, as in emoji sequences, but can also disguise it: they go.
        if (name.any { Character.isISOControl(it) }) return null
        return withoutHiddenCharacters(name).takeUnless { it.isBlank() }
    }

    /** RFC 3986 decoding: `%` needs two hex digits and `+` stays a plus. */
    private fun percentDecode(text: String): ByteArray? {
        val out = java.io.ByteArrayOutputStream(text.length)
        var i = 0
        while (i < text.length) {
            val c = text[i]
            if (c == '%') {
                if (i + 2 >= text.length) return null
                val hi = Character.digit(text[i + 1], 16)
                val lo = Character.digit(text[i + 2], 16)
                if (hi < 0 || lo < 0) return null
                out.write(hi * 16 + lo)
                i += 3
            } else {
                out.write(c.code)
                i++
            }
        }
        return out.toByteArray()
    }

    private fun addresses(text: String): List<InetSocketAddress>? {
        if (text.isEmpty()) return emptyList()
        val parts = text.split(',')
        if (parts.size > MAX_ADDRESSES) return null
        return parts.map { address(it) ?: return null }
    }

    private fun address(text: String): InetSocketAddress? {
        val host: String
        val port: Int
        when {
            text.startsWith('[') -> {
                val close = text.indexOf(']')
                if (close < 0) return null
                host = text.substring(1, close)
                val rest = text.substring(close + 1)
                port = when {
                    rest.isEmpty() -> DiscoveryConstants.BEACON_PORT
                    rest.startsWith(':') -> port(rest.substring(1)) ?: return null
                    else -> return null
                }
            }
            text.count { it == ':' } >= 2 -> {
                host = text
                port = DiscoveryConstants.BEACON_PORT
            }
            ':' in text -> {
                host = text.substringBefore(':')
                port = port(text.substringAfter(':')) ?: return null
            }
            else -> {
                host = text
                port = DiscoveryConstants.BEACON_PORT
            }
        }
        val bytes = (if (':' in host) ipv6(host) else ipv4(host)) ?: return null
        val address = InetAddress.getByAddress(bytes) // a literal, never a lookup
        if (address.isAnyLocalAddress || address.isLoopbackAddress || address.isMulticastAddress ||
            (bytes.size == 16 && address.isLinkLocalAddress)
        ) {
            return null
        }
        return InetSocketAddress(address, port)
    }

    private fun port(text: String): Int? {
        if (text.isEmpty() || text.length > 5 || !text.all { it in '0'..'9' }) return null
        return text.toInt().takeIf { it in 1..65535 }
    }

    /** Four decimal octets without leading zeros, which some parsers read as octal. */
    private fun ipv4(text: String): ByteArray? {
        val octets = text.split('.')
        if (octets.size != 4) return null
        return ByteArray(4) { i ->
            val o = octets[i]
            if (o.isEmpty() || o.length > 3 || !o.all { it in '0'..'9' } || (o.length > 1 && o[0] == '0')) return null
            o.toInt().takeIf { it <= 255 }?.toByte() ?: return null
        }
    }

    /** Colon-hex with at most one `::`; no zone and no embedded IPv4. */
    private fun ipv6(text: String): ByteArray? {
        if (text.isEmpty()) return null
        val doubleColon = text.indexOf("::")
        if (doubleColon >= 0 && text.indexOf("::", doubleColon + 1) >= 0) return null
        fun groups(part: String): List<Int>? {
            if (part.isEmpty()) return emptyList()
            return part.split(':').map { g ->
                if (g.isEmpty() || g.length > 4 || !g.all { Character.digit(it, 16) >= 0 }) return null
                g.toInt(16)
            }
        }
        val all: List<Int> = if (doubleColon < 0) {
            groups(text)?.takeIf { it.size == 8 } ?: return null
        } else {
            val head = groups(text.substring(0, doubleColon)) ?: return null
            val tail = groups(text.substring(doubleColon + 2)) ?: return null
            if (head.size + tail.size > 7) return null
            head + List(8 - head.size - tail.size) { 0 } + tail
        }
        return ByteArray(16) { i -> (all[i / 2] shr (if (i % 2 == 0) 8 else 0)).toByte() }
    }

    /**
     * 12 hex digits to "0C:1A:2B:3C:4D:5E". The address must be a public
     * one, which the desktop alone knows; the phone can only reject what no
     * public address looks like: locally administered, multicast, or all zero.
     */
    private fun bluetoothAddress(text: String): String? {
        if (text.length != 12 || !text.all { Character.digit(it, 16) >= 0 }) return null
        val bytes = ByteArray(6) { i -> text.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
        val first = bytes[0].toInt()
        if (first and 0x03 != 0 || bytes.all { it.toInt() == 0 }) return null
        return bytes.joinToString(":") { "%02X".format(it) }
    }
}
