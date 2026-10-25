package dev.link2linux.android.core.model

/**
 * What a phone and a desktop can share. Each feature has the same name on
 * every side: `feature/<name>` here, `src/features/<name>` on the desktop and
 * a module in `crates/core`.
 *
 * [wireName] is the string sent in `Hello.features`, so it must never change.
 */
enum class Feature(val wireName: String) {
    PAIRING("pairing"),
    NOTIFICATIONS("notifications"),
    MESSAGES("messages"),
    CALLS("calls"),
    PHOTOS("photos"),
    FILES("files"),
    CLIPBOARD("clipboard"),
    SCREEN("screen"),
    ;

    /**
     * Opt-in features stay off until the user turns them on, and only then
     * ask for their Android permissions. Pairing is the prerequisite for all
     * of them, so it is always available.
     */
    val isOptIn: Boolean
        get() = this != PAIRING

    companion object {
        /** The features a user can turn on one by one. */
        val optIn: List<Feature> = entries.filter(Feature::isOptIn)

        /** Looks up a feature by its wire name; unknown names (from newer peers) give null. */
        fun fromWireName(name: String): Feature? = entries.firstOrNull { it.wireName == name }
    }
}
