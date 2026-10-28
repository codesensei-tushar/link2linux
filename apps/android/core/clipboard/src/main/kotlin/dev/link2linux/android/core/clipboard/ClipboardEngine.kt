package dev.link2linux.android.core.clipboard

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ClipboardPush
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.clipboardPush
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.DelicateCoroutinesApi
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.security.MessageDigest

/**
 * The phone's side of the clipboard feature (docs/protocol.md, "Clipboard").
 *
 * A desktop's push goes on the phone's clipboard through [ClipboardWriter]
 * and is answered `Ack`; with `DISABLED` while the user has the feature off,
 * `INVALID_ARGUMENT` when the text is empty or over [MAX_TEXT_BYTES], and
 * `UNAVAILABLE` when the clipboard refuses it.
 *
 * Android lets an app read the clipboard only while it has focus, so the
 * phone shares when the user asks, through [share], and on its own when
 * the app gains focus (see [OpenShare]). A share goes to
 * every attached desktop whose `FeatureStatus` says `READY`. At most one
 * push per desktop waits for its answer: a newer share replaces one still
 * queued behind it, and only the newest is sent once the answer arrives,
 * if the feature is still on here and `READY` there. A `DISABLED` answer
 * counts as the desktop's status until its next `FeatureStatus`.
 *
 * The echo and relay rules hold for shares a device makes on its own: such
 * a share leaves out text received from any desktop, and text already pushed
 * to the desktop. The engine remembers both per desktop, by SHA-256 rather
 * than the text itself. A share the user asked for goes to every desktop.
 */
class ClipboardEngine(
    private val scope: CoroutineScope,
    private val writer: ClipboardWriter,
    /** Whether the user turned the feature on. */
    val enabled: StateFlow<Boolean>,
    private val onError: (Throwable) -> Unit = {},
) {

    private class Desktop(val peer: ClipboardPeer) {
        var availability = FeatureAvailability.FEATURE_AVAILABILITY_UNSPECIFIED

        /** A push to this desktop waits for its answer. */
        var sending = false

        /** The share to send once it has; a newer share replaces it. */
        var queued: Pending? = null

        /** Counts the desktop's `FeatureStatus` messages, so an answer can tell whether a newer one came. */
        var statusCount = 0
    }

    private class Pending(val clip: ClipText, val digest: ByteString) {
        val outcome = CompletableDeferred<PushOutcome>()
    }

    private val lock = Mutex()
    private val desktops = ArrayList<Desktop>() // guarded by lock, in the order they attached

    // Per desktop id: the text last received from it, and last pushed to it.
    private val received = HashMap<String, ByteString>() // guarded by lock
    private val pushed = HashMap<String, ByteString>() // guarded by lock

    private val _connected = MutableStateFlow<List<ConnectedDesktop>>(emptyList())

    /** The desktops a share would go to, with what their `FeatureStatus` says. */
    val connected: StateFlow<List<ConnectedDesktop>> = _connected.asStateFlow()

    /** A session with the feature active started. It replaces an older session with the same desktop. */
    suspend fun attach(peer: ClipboardPeer) = lock.withLock {
        desktops.filter { it.peer.desktopId == peer.desktopId }.forEach(::drop)
        desktops += Desktop(peer)
        publish()
    }

    /** The session ended. A share queued for it fails; one in flight fails with its connection. */
    suspend fun detach(peer: ClipboardPeer) = lock.withLock {
        desktops.firstOrNull { it.peer === peer }?.let(::drop)
        publish()
    }

    /** The desktop's `FeatureStatus` gives the feature [availability] there. */
    suspend fun onDesktopStatus(peer: ClipboardPeer, availability: FeatureAvailability) = lock.withLock {
        desktops.firstOrNull { it.peer === peer }?.let {
            it.availability = availability
            it.statusCount++
        }
        publish()
    }

    /** A push from the desktop [desktopId] names: puts it on the clipboard and returns the answer. */
    suspend fun receive(desktopId: String, push: ClipboardPush): ClipboardReply {
        if (!enabled.value) return failure(ErrorCode.ERROR_CODE_DISABLED, "Clipboard sharing is turned off on the phone")
        val size = push.textBytes.size()
        if (size == 0 || size > MAX_TEXT_BYTES) return failure(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, "text has $size bytes")
        try {
            writer.write(ClipText(push.text, push.sensitive))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            onError(e)
            return failure(ErrorCode.ERROR_CODE_UNAVAILABLE, "The phone's clipboard refused the text")
        }
        lock.withLock { received[desktopId] = digest(push.textBytes.toByteArray()) }
        return ClipboardReply.Ack
    }

    /**
     * Shares [clip] with the connected desktops and waits for their answers.
     *
     * @param automatic the user did not ask for this share. It then goes
     *   only to desktops that are ready, and not where the echo and relay
     *   rules forbid it; such desktops are left out of the result.
     */
    suspend fun share(clip: ClipText, automatic: Boolean = false): ShareResult {
        if (!enabled.value) return ShareResult.Disabled
        val bytes = clip.text.encodeToByteArray()
        val digest = digest(bytes)
        val waits = lock.withLock {
            when {
                desktops.isEmpty() -> return ShareResult.NoDesktop
                bytes.isEmpty() -> return ShareResult.Empty
                bytes.size > MAX_TEXT_BYTES -> return ShareResult.TooLarge
                automatic && digest in received.values -> return ShareResult.Pushed(emptyList())
            }
            desktops.mapNotNull { desktop ->
                val refusal = refusal(desktop)
                val outcome = when {
                    automatic && (refusal != null || pushed[desktop.peer.desktopId] == digest) -> return@mapNotNull null
                    refusal != null -> CompletableDeferred(refusal)
                    else -> enqueue(desktop, Pending(clip, digest))
                }
                desktop.peer.name to outcome
            }
        }
        return ShareResult.Pushed(waits.map { (name, outcome) -> DesktopOutcome(name, outcome.await()) })
    }

    /**
     * What a share would come to whatever its text, or null when it may
     * reach a desktop: the feature is off here, no desktop is connected, or
     * none is ready. Lets the phone skip reading the clipboard for nothing.
     */
    fun refusalBeforeSharing(): ShareResult? {
        if (!enabled.value) return ShareResult.Disabled
        val desktops = connected.value
        return when {
            desktops.isEmpty() -> ShareResult.NoDesktop
            desktops.any { it.isReady } -> null
            else -> ShareResult.Pushed(
                desktops.map {
                    val off = it.availability == FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
                    DesktopOutcome(it.name, if (off) PushOutcome.TURNED_OFF else PushOutcome.FAILED)
                },
            )
        }
    }

    /** Sends [pending] to [desktop], or queues it behind the push in flight there. Called under the lock. */
    @OptIn(DelicateCoroutinesApi::class)
    private fun enqueue(desktop: Desktop, pending: Pending): CompletableDeferred<PushOutcome> {
        if (desktop.sending) {
            desktop.queued?.outcome?.complete(PushOutcome.REPLACED)
            desktop.queued = pending
        } else {
            desktop.sending = true
            // Atomic: started even if the scope is cancelled meanwhile, so the share hears back.
            scope.launch(start = CoroutineStart.ATOMIC) { sendAll(desktop, pending) }
        }
        return pending.outcome
    }

    /** Sends [first], then what was queued for [desktop] meanwhile, one push at a time. */
    private suspend fun sendAll(desktop: Desktop, first: Pending) {
        var next: Pending? = first
        try {
            while (next != null) {
                val pending = next
                // The feature may have been turned off, here or there, while this waited.
                val (refusal, statusCount) = lock.withLock { refusal(desktop) to desktop.statusCount }
                val outcome = refusal ?: pushOne(desktop.peer, pending.clip)
                next = lock.withLock {
                    if (outcome == PushOutcome.SENT) pushed[desktop.peer.desktopId] = pending.digest
                    // The desktop answered DISABLED, and no newer FeatureStatus says otherwise.
                    if (refusal == null && outcome == PushOutcome.TURNED_OFF && desktop.statusCount == statusCount) {
                        desktop.availability = FeatureAvailability.FEATURE_AVAILABILITY_DISABLED
                        publish()
                    }
                    pending.outcome.complete(outcome)
                    desktop.queued.also {
                        desktop.queued = null
                        if (it == null) desktop.sending = false
                    }
                }
            }
        } catch (e: CancellationException) {
            // Cancelled with the engine's scope: nobody waits forever.
            next?.outcome?.complete(PushOutcome.FAILED)
            withContext(NonCancellable) {
                lock.withLock {
                    desktop.queued?.outcome?.complete(PushOutcome.FAILED)
                    desktop.queued = null
                    desktop.sending = false
                }
            }
            throw e
        }
    }

    private suspend fun pushOne(peer: ClipboardPeer, clip: ClipText): PushOutcome = try {
        val answer = peer.push(
            clipboardPush {
                text = clip.text
                sensitive = clip.sensitive
            },
        )
        when (answer) {
            PushAnswer.Ack -> PushOutcome.SENT
            is PushAnswer.Error ->
                if (answer.code == ErrorCode.ERROR_CODE_DISABLED) PushOutcome.TURNED_OFF else PushOutcome.FAILED
            PushAnswer.NoAnswer -> PushOutcome.FAILED
        }
    } catch (e: CancellationException) {
        throw e
    } catch (_: Exception) {
        // The connection failed, and its session ends.
        PushOutcome.FAILED
    }

    /** Why nothing may be pushed to [desktop] now, as the push's outcome, or null when a push may go. Called under the lock. */
    private fun refusal(desktop: Desktop): PushOutcome? = when {
        !enabled.value -> PushOutcome.FAILED
        desktop.availability == FeatureAvailability.FEATURE_AVAILABILITY_READY -> null
        desktop.availability == FeatureAvailability.FEATURE_AVAILABILITY_DISABLED -> PushOutcome.TURNED_OFF
        else -> PushOutcome.FAILED
    }

    /** Removes [desktop]; what is queued for it fails. Called under the lock. */
    private fun drop(desktop: Desktop) {
        desktops.remove(desktop)
        desktop.queued?.outcome?.complete(PushOutcome.FAILED)
        desktop.queued = null
    }

    /** Called under the lock. */
    private fun publish() {
        _connected.value = desktops.map { ConnectedDesktop(it.peer.name, it.availability) }
    }

    private fun digest(bytes: ByteArray): ByteString =
        ByteString.copyFrom(MessageDigest.getInstance("SHA-256").digest(bytes))
}

/** What became of a share, for the message the user sees. */
sealed interface ShareResult {
    /** The user has the feature off on this phone. */
    data object Disabled : ShareResult

    /** No connected desktop has the feature. */
    data object NoDesktop : ShareResult

    /** The text is empty. */
    data object Empty : ShareResult

    /** The text is over [MAX_TEXT_BYTES]. It is not sent, since a partial paste is worse than none. */
    data object TooLarge : ShareResult

    /** What became of it on each connected desktop. */
    data class Pushed(val outcomes: List<DesktopOutcome>) : ShareResult
}

data class DesktopOutcome(val name: String, val outcome: PushOutcome)

enum class PushOutcome {
    /** The desktop took the text. */
    SENT,

    /** The user has the feature off on that desktop. */
    TURNED_OFF,

    /** The desktop could not take it now, or the answer or the connection was lost. */
    FAILED,

    /** A newer share replaced it while it waited behind an earlier one. */
    REPLACED,
}

/** A connected desktop where the feature is active, and what its `FeatureStatus` says. */
data class ConnectedDesktop(val name: String, val availability: FeatureAvailability) {
    val isReady: Boolean get() = availability == FeatureAvailability.FEATURE_AVAILABILITY_READY
}
