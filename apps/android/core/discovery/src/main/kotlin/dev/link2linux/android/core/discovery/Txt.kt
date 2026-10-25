package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PresenceKey
import java.security.SecureRandom

/** What the phone announces, in the form `NsdServiceInfo` takes. */
data class Announcement(
    /** `l2l-` and 8 hex digits; Android may change it on a conflict. */
    val instanceName: String,
    /** TXT keys and values. */
    val txt: Map<String, String>,
)

/** Where randomness comes from; tests pass fixed bytes. */
fun interface RandomBytes {
    fun next(size: Int): ByteArray
}

/** [RandomBytes] backed by [SecureRandom]. */
class SecureRandomBytes(private val random: SecureRandom = SecureRandom()) : RandomBytes {
    override fun next(size: Int): ByteArray = ByteArray(size).also(random::nextBytes)
}

/** What the phone is doing, as far as announcements are concerned. */
data class AnnounceState(
    /**
     * Presence keys of the desktops to announce to, the one the phone was
     * connected to most recently first. Only the first three get a tag.
     */
    val presenceKeys: List<PresenceKey> = emptyList(),
    /** The secret of the QR code scanned while waiting for that desktop. */
    val pairingSecret: PairingSecret? = null,
    /** "Pair without camera" is on. */
    val sasMode: Boolean = false,
)

/** Builds the instance name and TXT record of the mDNS registration. */
object Txt {

    fun instanceName(random: ByteArray): String {
        require(random.size == 4) { "An instance name takes 4 random bytes" }
        return DiscoveryConstants.INSTANCE_PREFIX + random.joinToString("") { "%02x".format(it) }
    }

    /**
     * The TXT record for [state] with the presence [nonce]. `n` and the tags
     * appear only when the phone has a pairing; `q` and `pm` only while the
     * phone waits for a desktop.
     */
    fun build(state: AnnounceState, nonce: ByteArray): Map<String, String> {
        val txt = linkedMapOf("v" to DiscoveryConstants.PROTOCOL_MAJOR.toString())
        val keys = state.presenceKeys.take(DiscoveryConstants.MAX_PRESENCE_TAGS)
        if (keys.isNotEmpty()) {
            txt["n"] = nonce.toBase64Url()
            keys.forEachIndexed { i, key ->
                txt[if (i == 0) "t" else "t${i + 1}"] = Tags.presence(key, nonce).toBase64Url()
            }
        }
        state.pairingSecret?.let { txt["q"] = Tags.pairing(it).toBase64Url() }
        if (state.sasMode) txt["pm"] = "1"
        check(sizeInBytes(txt) < DiscoveryConstants.MAX_TXT_BYTES) { "TXT record too large" }
        return txt
    }

    /** The size of [txt] on the wire: each entry is a length byte and `key=value`. */
    fun sizeInBytes(txt: Map<String, String>): Int =
        txt.entries.sumOf { (k, v) -> 1 + k.toByteArray().size + 1 + v.toByteArray().size }
}
