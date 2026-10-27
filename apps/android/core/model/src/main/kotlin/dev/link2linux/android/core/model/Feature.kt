package dev.link2linux.android.core.model

/**
 * What a phone and a desktop can share. Each feature has the same name on
 * every side: `feature/<name>` here, `src/features/<name>` on the desktop and
 * a module in `crates/core`.
 *
 * [wireName] is the string sent in `Hello.features`, so it must never change.
 * `messages` was a planned feature that was dropped; its wire name stays
 * reserved and must not be given to another feature.
 */
enum class Feature(val wireName: String) {
    PAIRING("pairing"),
    NOTIFICATIONS("notifications"),
    CALLS("calls"),
    PHOTOS("photos"),
    FILES("files"),
    CLIPBOARD("clipboard"),
    SCREEN("screen"),
    STATUS("status"),
    ;

    /**
     * Whether the user can turn the feature on and off. Pairing is the
     * prerequisite for all the others, so it is always on.
     */
    val isSwitchable: Boolean
        get() = this != PAIRING

    /**
     * Whether the feature is on until the user turns it off. Most features
     * share what is on the phone, so they are opt-in: off until the user
     * turns them on, and only then ask for their Android permissions. Calls
     * and phone status share nothing by themselves and need no permission,
     * so they start on.
     */
    val isOnByDefault: Boolean
        get() = this == CALLS || this == STATUS

    companion object {
        /** The features a user can turn on and off, each on its own screen. */
        val switchable: List<Feature> = entries.filter(Feature::isSwitchable)

        /** Looks up a feature by its wire name; unknown names (from newer peers) give null. */
        fun fromWireName(name: String): Feature? = entries.firstOrNull { it.wireName == name }
    }
}
