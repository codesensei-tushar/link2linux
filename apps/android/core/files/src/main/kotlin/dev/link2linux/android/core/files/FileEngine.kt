package dev.link2linux.android.core.files

import dev.link2linux.protocol.v1.fileCancel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import java.security.SecureRandom

/**
 * Sends and receives files (docs/protocol.md, "Files").
 *
 * Each session with a desktop where files are active is wired with
 * [attach], [handle] for each of the desktop's requests, and [detach]. The
 * user sends files with [send] and stops a transfer with [cancel]; what
 * they see is [transfers]. [availability] feeds `FeatureStatus`, and the
 * Android side calls [onPolicyChanged] when the user's choices change and
 * [sweep] every minute or so.
 *
 * Transfers live as long as the process. A transfer cut short by a lost
 * connection is kept for [KEEP_MILLIS] and goes on when its desktop is back:
 * an outgoing one offers itself again, an incoming one waits for the
 * desktop's offer. Partial files of an earlier run are deleted before the
 * first offer is accepted.
 */
class FileEngine(
    private val scope: CoroutineScope,
    private val policy: FilePolicy,
    store: ReceivedFileStore,
    clock: () -> Long = System::currentTimeMillis,
    random: SecureRandom = SecureRandom(),
    private val onError: (Throwable) -> Unit = {},
) {
    private val _availability = MutableStateFlow(current())

    /** Whether the feature can be used now, for `FeatureStatus`. */
    val availability: StateFlow<FileAvailability> = _availability.asStateFlow()

    private val board = TransferBoard()

    /** The running transfers and the latest finished ones, newest first. */
    val transfers: StateFlow<List<Transfer>> = board.state

    /** The latest session with each desktop, by peer id. Guarded by itself. */
    private val links = HashMap<String, FileLink>()

    /** Per attached session, the parent of what waits on it; cancelled when it ends. Guarded by [links]. */
    private val linkJobs = HashMap<FileLink, Job>()

    /** `FileCancel`s that could not go out yet, by peer id. Guarded by itself. */
    private val pendingCancels = HashMap<String, MutableSet<TransferId>>()

    private val cleaned = scope.async { runCatching(store::removeLeftovers).onFailure(onError) }

    private val receiver = FileReceiver(store, board, clock, { _availability.value }, ::sendCancel, onError)
    private val sender = FileSender(scope, board, clock, random, _availability, ::sendCancel, onError)

    /** A session with [FileLink.peer] started, and files are active in it. */
    fun attach(link: FileLink) {
        synchronized(links) {
            links[link.peer.id] = link
            linkJobs[link] = SupervisorJob(scope.coroutineContext[Job])
        }
        sender.attach(link)
        val owed = synchronized(pendingCancels) { pendingCancels.remove(link.peer.id) }.orEmpty()
        owed.forEach { sendCancel(link, link.peer, it) }
    }

    /** The session [link] ended. Its transfers wait for the desktop to come back. */
    suspend fun detach(link: FileLink) {
        synchronized(links) {
            if (links[link.peer.id] === link) links.remove(link.peer.id)
            linkJobs.remove(link)
        }?.cancel()
        sender.detach(link)
        receiver.detach(link)
    }

    /** Answers a request of the desktop of [link]. */
    suspend fun handle(link: FileLink, request: FileRequest): FileAnswer = when (request) {
        is FileRequest.Offer -> {
            cleaned.await()
            receiver.offer(link, request.message)
        }
        is FileRequest.Chunk -> receiver.chunk(link, request.message)
        is FileRequest.Cancel -> {
            // Ids are the sender's, so the transfer may go either way. Unknown ones are fine too.
            val id = TransferId(request.message.transferId)
            receiver.cancelledByPeer(link, id)
            sender.cancelledByPeer(link, id)
            FileAnswer.Ack
        }
    }

    /**
     * Sends [files] to [peer], as soon as a session with it is up and files
     * are ready on both sides, in transfers of at most [MAX_FILES] files.
     */
    fun send(peer: FilePeer, files: List<OutgoingFile>): List<TransferKey> = sender.send(peer, files)

    /** The user stops a transfer. Its desktop is told, now or when it is back. */
    suspend fun cancel(key: TransferKey) = when (key.direction) {
        Direction.INCOMING -> receiver.cancel(key)
        Direction.OUTGOING -> sender.cancel(key)
    }

    /** Takes a finished transfer off [transfers]. */
    fun dismiss(key: TransferKey) = board.remove(key)

    /** The user's choices changed. Turning the feature off stops every transfer. */
    fun onPolicyChanged() {
        val now = current()
        _availability.value = now
        if (now != FileAvailability.READY) {
            scope.launch {
                receiver.cancelAll()
                sender.cancelAll()
            }
        }
    }

    /** Ends what was kept for [KEEP_MILLIS] without progress. */
    suspend fun sweep() {
        receiver.sweep()
        sender.sweep()
    }

    private fun current(): FileAvailability = when {
        !policy.enabled -> FileAvailability.DISABLED
        !policy.storageGranted -> FileAvailability.PERMISSION_REQUIRED
        else -> FileAvailability.READY
    }

    /**
     * Sends `FileCancel` for [id] on [link], or on the desktop's current
     * session, once files are ready on its side; with neither, when the
     * desktop is back.
     */
    private fun sendCancel(link: FileLink?, peer: FilePeer, id: TransferId) {
        val (to, job) = synchronized(links) {
            val target = link ?: links[peer.id]
            Pair(target, target?.let(linkJobs::get))
        }
        if (to == null || job == null) {
            owe(peer, id)
            return
        }
        // A child of the session's job: if the session ends first, the cancel waits for the next one.
        scope.launch(job) {
            var sent: PendingAnswer? = null
            try {
                to.peerReady.first { it }
                sent = to.send(FileRequest.Cancel(fileCancel { transferId = id.bytes }))
                sent.await()
            } catch (e: Exception) {
                if (sent == null) owe(peer, id)
                if (e is CancellationException) throw e
            }
        }
    }

    private fun owe(peer: FilePeer, id: TransferId) {
        synchronized(pendingCancels) { pendingCancels.getOrPut(peer.id) { mutableSetOf() } += id }
    }

    companion object {
        /** The permission token of shared storage access, which Android 8 and 9 need. */
        const val STORAGE_PERMISSION = "storage"

        /** Files per offer. */
        const val MAX_FILES = 500

        /** Bytes of data per chunk. */
        const val MAX_CHUNK_BYTES = 256 * 1024

        /** Chunks a sender may have without an answer, per session. */
        const val MAX_UNANSWERED_CHUNKS = 8

        /** Transfers from one desktop that run at once. */
        const val MAX_RUNNING_PER_PEER = 4

        /** How long an unfinished transfer is kept without progress. */
        const val KEEP_MILLIS = 60 * 60 * 1000L

        /** Room a received transfer leaves free, so the phone never fills up. */
        const val SPARE_BYTES = 64L * 1024 * 1024
    }
}

/** The transfers the user sees: the unfinished ones, and the latest finished ones. */
internal class TransferBoard(private val maxFinished: Int = MAX_FINISHED) {
    private val entries = LinkedHashMap<TransferKey, Transfer>() // guarded by this, in the order they came
    private val _state = MutableStateFlow<List<Transfer>>(emptyList())
    val state: StateFlow<List<Transfer>> = _state.asStateFlow()

    fun put(transfer: Transfer) = synchronized(this) {
        entries[transfer.key] = transfer
        val finished = entries.values.filter { it.state.isFinished }
        if (finished.size > maxFinished) {
            finished.sortedBy { it.createdUnixMs }.take(finished.size - maxFinished).forEach { entries.remove(it.key) }
        }
        publish()
    }

    /** Removes [key] if it is finished. */
    fun remove(key: TransferKey) = synchronized(this) {
        if (entries[key]?.state?.isFinished == true) {
            entries.remove(key)
            publish()
        }
    }

    private fun publish() {
        _state.value = entries.values.sortedByDescending { it.createdUnixMs }
    }

    private companion object {
        const val MAX_FINISHED = 50
    }
}
