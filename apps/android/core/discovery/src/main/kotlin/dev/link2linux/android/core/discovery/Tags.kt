package dev.link2linux.android.core.discovery

import dev.link2linux.android.core.model.PairingSecret
import dev.link2linux.android.core.model.PresenceKey
import java.util.Base64
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** base64url without padding, as every discovery value is written. */
internal fun ByteArray.toBase64Url(): String = Base64.getUrlEncoder().withoutPadding().encodeToString(this)

internal fun hmacSha256(key: ByteArray, vararg parts: ByteArray): ByteArray {
    val mac = Mac.getInstance("HmacSHA256")
    mac.init(SecretKeySpec(key, "HmacSHA256"))
    parts.forEach(mac::update)
    return mac.doFinal()
}

/** The tags that let a desktop recognize a phone without a stable identifier. */
object Tags {

    const val NONCE_SIZE: Int = 8
    const val TAG_SIZE: Int = 8

    private val presenceLabel = "l2l-presence-v1".toByteArray(Charsets.US_ASCII)
    private val pairingLabel = "l2l-pairing-tag-v1".toByteArray(Charsets.US_ASCII)

    /** `HMAC-SHA256(presence_key, "l2l-presence-v1" || nonce)`, first 8 bytes. */
    fun presence(key: PresenceKey, nonce: ByteArray): ByteArray {
        require(nonce.size == NONCE_SIZE) { "A presence nonce has $NONCE_SIZE bytes" }
        return hmacSha256(key.toByteArray(), presenceLabel, nonce).copyOf(TAG_SIZE)
    }

    /** `HMAC-SHA256(secret, "l2l-pairing-tag-v1")`, first 8 bytes. */
    fun pairing(secret: PairingSecret): ByteArray =
        hmacSha256(secret.toByteArray(), pairingLabel).copyOf(TAG_SIZE)
}
