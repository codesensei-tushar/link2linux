package dev.link2linux.android.core.discovery

/** Fixed values of the discovery part of the protocol (docs/protocol.md, "Constants"). */
object DiscoveryConstants {
    /** The DNS-SD service type the phone registers; Android wants it without `.local.`. */
    const val SERVICE_TYPE: String = "_link2linux._tcp"

    /** The TCP port the phone listens on unless it is taken. */
    const val DEFAULT_PHONE_PORT: Int = 27380

    /** The UDP port the desktop listens on for beacons. */
    const val BEACON_PORT: Int = 27381

    /** The Bluetooth LE service UUID the desktop advertises. */
    const val BLE_SERVICE_UUID: String = "d0135d8c-397d-4afe-b65f-bb4c33308f6d"

    /** The highest protocol major version, advertised in the `v` TXT key. */
    const val PROTOCOL_MAJOR: Int = 1

    /** An mDNS instance name is this prefix and 8 hex digits. */
    const val INSTANCE_PREFIX: String = "l2l-"

    /** The most presence tags a TXT record carries: `t`, `t2` and `t3`. */
    const val MAX_PRESENCE_TAGS: Int = 3

    /** A TXT record stays below this many bytes. */
    const val MAX_TXT_BYTES: Int = 200
}
