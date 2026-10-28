package dev.link2linux.android.core.clipboard

import kotlinx.coroutines.flow.StateFlow

/**
 * Decides when opening the app shares the clipboard on its own
 * (docs/protocol.md, "Clipboard").
 *
 * Android lets the phone read the clipboard only while the app has focus,
 * so the phone shares a copy the user made in another app once the app
 * gains focus. Before reading a copy, the phone knows only when it was made
 * and what kind it is, and on Android 12 and later every first read of
 * another app's copy shows a notice. So each copy is read at most once: a
 * copy is new while its time differs from that of the last copy handled,
 * whether that one was shared on its own, shared on the user's ask, or
 * left out. [store] keeps that time across restarts of the app, so an old
 * copy does not go out again.
 *
 * A copy its source app marked sensitive is left out: the phone shares
 * secrets only when the user asks. So is text a computer sent: the engine
 * already leaves it out, but forgets it when the app restarts.
 *
 * Not thread-safe: the app calls it on the main thread, where it gains
 * focus and reads the clipboard.
 */
class OpenShare(
    private val engine: ClipboardEngine,
    /** Whether the user lets opening the app share the clipboard. */
    private val enabled: StateFlow<Boolean>,
    private var handled: Long,
    private val store: (Long) -> Unit,
) {
    /**
     * Whether to read the copy made at [copiedAtMillis] and share it on the
     * phone's own, given whether it [isText], is [sensitive], and came
     * [fromComputer]. While the feature is off or no computer would take
     * the copy, it stays new, so the app shares it once one does.
     */
    fun shouldRead(copiedAtMillis: Long, isText: Boolean, sensitive: Boolean, fromComputer: Boolean): Boolean {
        if (!enabled.value || engine.refusalBeforeSharing() != null || copiedAtMillis == handled) return false
        handled(copiedAtMillis)
        return isText && !sensitive && !fromComputer
    }

    /** Records the copy made at [copiedAtMillis] as handled, as when the user shared it. */
    fun handled(copiedAtMillis: Long) {
        if (copiedAtMillis == handled) return
        handled = copiedAtMillis
        store(copiedAtMillis)
    }
}
