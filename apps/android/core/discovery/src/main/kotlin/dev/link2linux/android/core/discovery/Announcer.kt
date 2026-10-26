package dev.link2linux.android.core.discovery

/**
 * Keeps the presence nonce and the instance name that go with it, and builds
 * the [Announcement] for the phone's current state.
 *
 * The nonce and name change when the phone joins a network and about every
 * 15 minutes ([rotate]). They stay fixed while the phone is in SAS pairing
 * mode, so the name the user compares on the desktop does not change under
 * them.
 */
class Announcer(private val random: RandomBytes = SecureRandomBytes()) {

    private var nonce: ByteArray = random.next(Tags.NONCE_SIZE)
    private var instanceName: String = Txt.instanceName(random.next(4))

    /** Picks a new nonce and instance name, unless [sasMode] is on. */
    fun rotate(sasMode: Boolean = false) {
        if (sasMode) return
        nonce = random.next(Tags.NONCE_SIZE)
        instanceName = Txt.instanceName(random.next(4))
    }

    fun announcement(state: AnnounceState): Announcement =
        Announcement(instanceName, Txt.build(state, nonce))

    companion object {
        /** How often the nonce rotates while the phone stays on one network. */
        const val ROTATE_MILLIS: Long = 15 * 60_000
    }
}
