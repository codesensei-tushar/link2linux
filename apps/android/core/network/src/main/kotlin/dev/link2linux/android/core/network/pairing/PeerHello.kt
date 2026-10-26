package dev.link2linux.android.core.network.pairing

import dev.link2linux.android.core.model.withoutHiddenCharacters
import dev.link2linux.android.core.notifications.WireText
import dev.link2linux.protocol.v1.Hello

/**
 * The limits a desktop's `Hello` must keep (docs/protocol.md, "Identity" and
 * "Capabilities"). Trust never depends on these fields; the limits keep a
 * peer from making the phone store, show or scan oversized or misleading
 * values.
 */
internal object PeerHello {

    const val MAX_NAME_BYTES: Int = 64
    const val MAX_FEATURES: Int = 64
    const val MAX_FEATURE_BYTES: Int = 32
    const val MAX_APP_VERSION_BYTES: Int = 32

    /** What a device id may hold. Desktops so far send `desktop-` and 32 hex digits, not the UUID the spec names. */
    private val DEVICE_ID = Regex("[A-Za-z0-9-]{1,64}")

    /**
     * [hello] with its name cleaned and cut to [MAX_NAME_BYTES] and its app
     * version cut to [MAX_APP_VERSION_BYTES].
     *
     * @throws IllegalArgumentException if the device id is not 1 to 64 ASCII
     *   letters, digits and hyphens, or the features break their limits.
     */
    fun checked(hello: Hello): Hello {
        require(DEVICE_ID.matches(hello.deviceId)) { "Hello.device_id is not 1 to 64 letters, digits and hyphens" }
        require(hello.featuresCount <= MAX_FEATURES) { "Hello lists ${hello.featuresCount} features" }
        require(hello.featuresList.all { it.toByteArray().size <= MAX_FEATURE_BYTES }) { "A Hello feature is too long" }
        val name = WireText.truncate(cleanName(hello.deviceName), MAX_NAME_BYTES)
        val version = WireText.truncate(hello.appVersion, MAX_APP_VERSION_BYTES)
        if (name == hello.deviceName && version == hello.appVersion) return hello
        return hello.toBuilder().setDeviceName(name).setAppVersion(version).build()
    }

    /** [name] without the characters that could disguise it on screen: see [withoutHiddenCharacters]. */
    fun cleanName(name: String): String = withoutHiddenCharacters(name)
}
