package dev.link2linux.android.core.notifications

import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** How the phone names a notification (`key`) and its group (`group`) on the wire. */
interface NotificationKeys {
    fun key(platformKey: String): String

    fun group(groupKey: String): String
}

/**
 * Opaque names: `k:` or `g:` and 22 base64url characters of an HMAC-SHA256
 * of the platform's key under [secret]. Android builds its keys from parts
 * the posting app chooses, such as a contact or an account in the tag, so
 * they are never sent as they are. The names stay the same for as long as
 * the secret does; the engine maps them back.
 */
class HmacNotificationKeys(secret: ByteArray) : NotificationKeys {
    private val key = SecretKeySpec(secret.copyOf(), ALGORITHM)

    override fun key(platformKey: String): String = "k:" + mac("key", platformKey)

    override fun group(groupKey: String): String = "g:" + mac("group", groupKey)

    private fun mac(kind: String, value: String): String {
        val mac = Mac.getInstance(ALGORITHM).apply { init(key) }
        mac.update(kind.toByteArray(Charsets.UTF_8))
        mac.update(0)
        // 128 bits: no two of a phone's notifications share a name.
        return base64Url.encodeToString(mac.doFinal(value.toByteArray(Charsets.UTF_8)).copyOf(16))
    }

    companion object {
        const val SECRET_SIZE: Int = 32

        private const val ALGORITHM = "HmacSHA256"
        private val base64Url = Base64.getUrlEncoder().withoutPadding()

        /** Names with a new secret, which last until the process ends. */
        fun random(): HmacNotificationKeys = HmacNotificationKeys(ByteArray(SECRET_SIZE).also(SecureRandom()::nextBytes))
    }
}
