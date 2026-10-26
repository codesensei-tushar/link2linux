package dev.link2linux.android.feature.files

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Where a share stands in the share target. */
sealed interface ShareState<out P, out D> {
    data object Idle : ShareState<Nothing, Nothing>

    /** Its documents are being read and described. */
    data object Preparing : ShareState<Nothing, Nothing>

    /** Ready: the user sends them, or not. */
    data class Ready<P>(val prepared: P) : ShareState<P, Nothing>

    /** Sent to [desktop], or let go when that is null. */
    data class Done<D>(val desktop: D?) : ShareState<Nothing, D>
}

/**
 * One share in the share target, from reading its documents to the user's
 * answer. Another app can start the share target without the user, so
 * nothing is sent before [confirm]; a share that is cancelled, or left
 * without an answer, stops being read, and is let go if it was read already.
 *
 * @param U a shared document. @param P the documents, read. @param D a desktop.
 */
@OptIn(ExperimentalCoroutinesApi::class) // Deferred.getCompleted, once it completed
internal class ShareFlow<U, P, D>(
    private val scope: CoroutineScope,
    /** Reads and describes the documents; a cancelled read stops, and lets go of what it took. */
    private val read: (List<U>) -> Deferred<P>,
    private val send: (D, P) -> Unit,
    private val discard: (P) -> Unit,
) {
    private val _state = MutableStateFlow<ShareState<P, D>>(ShareState.Idle)
    val state: StateFlow<ShareState<P, D>> = _state.asStateFlow()

    private var preparing: Deferred<P>? = null
    private var sent = false

    /** Reads the shared [documents]; only the first call does. */
    fun prepare(documents: List<U>) {
        if (!_state.compareAndSet(ShareState.Idle, ShareState.Preparing)) return
        val job = read(documents)
        preparing = job
        scope.launch {
            val prepared = job.await()
            _state.compareAndSet(ShareState.Preparing, ShareState.Ready(prepared))
        }
    }

    /** The user sends the documents to [desktop]. */
    fun confirm(desktop: D) {
        val ready = _state.value as? ShareState.Ready ?: return
        sent = true
        _state.value = ShareState.Done(desktop)
        send(desktop, ready.prepared)
    }

    fun cancel() {
        if (_state.value is ShareState.Done) return
        _state.value = ShareState.Done(null)
        discardWhenRead()
    }

    /** The share target went away without an answer: nothing goes. */
    fun close() {
        if (!sent) discardWhenRead()
    }

    /**
     * Stops reading the documents, and lets them go if they were read. A read
     * that is cancelled lets go of what it holds itself. Twice does no harm.
     */
    private fun discardWhenRead() {
        val job = preparing ?: return
        job.cancel()
        job.invokeOnCompletion { failure -> if (failure == null) discard(job.getCompleted()) }
    }
}
