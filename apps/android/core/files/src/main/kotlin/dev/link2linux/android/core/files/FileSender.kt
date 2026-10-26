package dev.link2linux.android.core.files

import com.google.protobuf.ByteString
import dev.link2linux.android.core.files.FileEngine.Companion.KEEP_MILLIS
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_CHUNK_BYTES
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_FILES
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_RUNNING_PER_PEER
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_UNANSWERED_CHUNKS
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FileInfo
import dev.link2linux.protocol.v1.fileChunk
import dev.link2linux.protocol.v1.fileInfo
import dev.link2linux.protocol.v1.fileOffer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.withContext
import java.io.IOException
import java.io.InputStream
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicLong

/**
 * The sending half of the engine: offers the user's files to a desktop and
 * sends their bytes.
 *
 * Each attached session runs a pump while files are `READY` on both sides.
 * It starts the desktop's waiting transfers, at most [MAX_RUNNING_PER_PEER]
 * at once, and they share one window of [MAX_UNANSWERED_CHUNKS] chunks
 * without an answer. A transfer reads its files itself, one chunk after the
 * other, so its chunks go out in order; their answers are awaited
 * alongside.
 *
 * A transfer whose session ends waits for the next one and offers itself
 * again with the same id: the desktop's `FileAccept` says where to go on,
 * and the hash of each file then covers the bytes the desktop already has,
 * read again from the start.
 */
internal class FileSender(
    private val scope: CoroutineScope,
    private val board: TransferBoard,
    private val clock: () -> Long,
    private val random: SecureRandom,
    private val availability: StateFlow<FileAvailability>,
    /** Tells the desktop that the transfer ended here; sends it later if [FileLink] is null or gone. */
    private val sendCancel: (FileLink?, FilePeer, TransferId) -> Unit,
    private val onError: (Throwable) -> Unit,
) {

    private inner class Outgoing(
        val key: TransferKey,
        val peer: FilePeer,
        val files: List<OutgoingFile>,
        val infos: List<FileInfo>,
    ) {
        val createdUnixMs = clock()
        private val totalBytes = infos.sumOf { it.size }
        val acked = AtomicLong()

        /** Answers the desktop gave to this transfer's requests; any answer ends a row of unanswered ones. */
        val answers = AtomicLong()

        // Guarded by the sender, which also shows every change on the board under its lock, so
        // the board never goes back to an older state.
        var state: TransferState = TransferState.Waiting
        var job: Job? = null
        var link: FileLink? = null

        /** An offer went out, so the desktop may hold part of the files. */
        @Volatile var offered = false

        /** The desktop accepted it once, so a later offer resumes it. */
        @Volatile var accepted = false

        /** A session that failed this transfer; it waits for another. */
        var stuckOn: FileLink? = null

        /** The last progress or interruption; an idle transfer is kept for [KEEP_MILLIS] after. */
        var since = createdUnixMs

        val running: Boolean get() = job?.isActive == true

        fun show() = board.put(
            Transfer(key, peer, infos.map { it.name }, totalBytes, acked.get(), state, emptyList(), createdUnixMs),
        )
    }

    /** How one attempt at a transfer ended. */
    private sealed interface Outcome {
        data object Done : Outcome

        /** A request got no answer in time: offer it again, unless that was the third in a row. */
        data object Unanswered : Outcome

        /** The desktop cannot take it yet: offer it again after [delayMillis], as often as it takes. */
        data class Later(val delayMillis: Long) : Outcome

        /** The desktop failed unexpectedly: offer it again a few times, waiting longer each time. */
        data object Failed : Outcome
    }

    private class PeerEndedException(val code: ErrorCode) : Exception("The desktop answered $code")

    private class FileReadException(val changed: Boolean, cause: Throwable? = null) :
        Exception(if (changed) "The file changed size" else "The file cannot be read", cause)

    private val transfers = LinkedHashMap<TransferKey, Outgoing>() // guarded by this, in the order they were sent
    private val pumps = HashMap<FileLink, Job>() // guarded by this
    private val windows = HashMap<FileLink, Window>() // guarded by this

    /** Desktops with a session where files are ready on both sides, and those sessions. Guarded by this. */
    private val readyLinks = HashMap<String, MutableList<FileLink>>()

    /** Bumped when a transfer may start: one was added, or one stopped. */
    private val wake = MutableStateFlow(0L)

    /** Queues [files] for [peer], in transfers of at most [MAX_FILES] files. */
    fun send(peer: FilePeer, files: List<OutgoingFile>): List<TransferKey> {
        val keys = files.chunked(MAX_FILES).map { group ->
            val infos = group.map { file ->
                fileInfo {
                    name = FileNames.safe(file.name)
                    size = file.size
                    mimeType = file.mimeType
                    modifiedUnixMs = file.modifiedUnixMs
                }
            }
            val transfer = Outgoing(TransferKey(Direction.OUTGOING, peer.id, TransferId.random(random)), peer, group, infos)
            synchronized(this) {
                transfers[transfer.key] = transfer
                transfer.show()
            }
            transfer.key
        }
        poke()
        return keys
    }

    /** Starts a pump for the session [link]. */
    fun attach(link: FileLink) {
        val window = Window()
        val pump = scope.launch(start = CoroutineStart.LAZY) {
            combine(link.peerReady, availability) { ready, here -> ready && here == FileAvailability.READY }
                .distinctUntilChanged()
                .collectLatest { ready ->
                    if (!ready) return@collectLatest
                    setReady(link, true)
                    try {
                        // Transfers run as children: when either side stops being ready, they stop and wait.
                        coroutineScope { wake.collect { startWaiting(link, window, this) } }
                    } finally {
                        setReady(link, false)
                    }
                }
        }
        synchronized(this) {
            windows.put(link, window)?.close()
            pumps.put(link, pump)
        }?.cancel()
        pump.start()
    }

    /** The session [link] ended: its transfers wait for the next one, and its chunks get no answers. */
    fun detach(link: FileLink) {
        val (pump, window) = synchronized(this) {
            transfers.values.forEach { if (it.stuckOn === link) it.stuckOn = null }
            pumps.remove(link) to windows.remove(link)
        }
        pump?.cancel()
        window?.close()
    }

    /**
     * One session's room for chunks without an answer, shared by its
     * transfers. A chunk keeps its place until the desktop answers it, its
     * answer times out or the session ends; not when its transfer stops,
     * since the chunk is still on its way and the desktop answers it all the
     * same. So a cancel, a timeout or a pause never lets more than
     * [MAX_UNANSWERED_CHUNKS] be on their way.
     */
    private inner class Window {
        val places = Semaphore(MAX_UNANSWERED_CHUNKS)

        /** Waits for the answers apart from the transfers; ended with the session. */
        private val waiting = CoroutineScope(scope.coroutineContext + SupervisorJob(scope.coroutineContext[Job]))

        /** Waits for [pending]'s answer and frees its place then; the transfer reads it if it still wants it. */
        fun answerOf(pending: PendingAnswer): Deferred<FileAnswer> {
            val answer = CompletableDeferred<FileAnswer>()
            // ATOMIC: it starts even if the session is ending, so the place is always freed.
            waiting.launch(start = CoroutineStart.ATOMIC) {
                try {
                    answer.complete(pending.await())
                } catch (e: CancellationException) {
                    answer.completeExceptionally(IOException("The session ended"))
                    throw e
                } catch (e: Exception) {
                    answer.completeExceptionally(e)
                } finally {
                    places.release()
                }
            }
            return answer
        }

        fun close() = waiting.cancel()
    }

    private fun startWaiting(link: FileLink, window: Window, runner: CoroutineScope) {
        val started = synchronized(this) {
            val mine = transfers.values.filter { it.peer.id == link.peer.id }
            val free = MAX_RUNNING_PER_PEER - mine.count { it.running }
            mine.filter { (it.state == TransferState.Waiting || it.state == TransferState.Interrupted) && !it.running && it.stuckOn !== link }
                .take(free.coerceAtLeast(0))
                .map { transfer ->
                    runner.launch(start = CoroutineStart.LAZY) { run(transfer, link, window) }.also { job ->
                        transfer.job = job
                        // Once it is no longer running, the next may start.
                        job.invokeOnCompletion { poke() }
                    }
                }
        }
        started.forEach { it.start() }
    }

    private suspend fun run(transfer: Outgoing, link: FileLink, window: Window) {
        var unanswered = 0
        var failures = 0
        try {
            while (true) {
                val answersBefore = transfer.answers.get()
                val outcome = try {
                    attempt(transfer, link, window)
                } catch (e: AnswerTimeoutException) {
                    Outcome.Unanswered
                }
                val wait = when (outcome) {
                    Outcome.Done -> return
                    Outcome.Unanswered -> {
                        unanswered = if (transfer.answers.get() > answersBefore) 1 else unanswered + 1
                        // The session looks stuck: the next one resumes the transfer.
                        if (unanswered >= MAX_UNANSWERED) return interrupt(transfer, stuckOn = link)
                        RETRY_DELAY_MILLIS
                    }
                    is Outcome.Later -> {
                        // The desktop is there and answers: waiting for it uses none of the hour.
                        synchronized(this) { transfer.since = clock() }
                        outcome.delayMillis
                    }
                    Outcome.Failed -> {
                        if (++failures > MAX_FAILURES) return fail(transfer, FailureReason.PEER_ERROR)
                        RETRY_DELAY_MILLIS shl (failures - 1)
                    }
                }
                setState(transfer, TransferState.Interrupted)
                delay(wait)
            }
        } catch (e: CancellationException) {
            // Cancelled by the user, or the pump stopped: the session ended or a side is no longer ready.
            interrupt(transfer, stuckOn = null)
            throw e
        } catch (e: IOException) {
            // The connection is gone. The next session resumes it.
            interrupt(transfer, stuckOn = link)
        } catch (e: Exception) {
            onError(e)
            fail(transfer, FailureReason.INTERNAL)
            if (transfer.offered) sendCancel(link, transfer.peer, transfer.key.id)
        }
    }

    /** Offers [transfer] on [link] and sends what the desktop does not have yet. */
    private suspend fun attempt(transfer: Outgoing, link: FileLink, window: Window): Outcome {
        synchronized(this) {
            if (transfer.state.isFinished) return Outcome.Done
            transfer.state = TransferState.Running
            transfer.link = link
            transfer.show()
        }
        val offer = fileOffer {
            transferId = transfer.key.id.bytes
            createdUnixMs = transfer.createdUnixMs
            files += transfer.infos
        }
        val pending = link.send(FileRequest.Offer(offer))
        // Even if its answer is lost, the desktop may have taken it.
        transfer.offered = true
        val answer = pending.await()
        transfer.answers.incrementAndGet()
        val accept = when (answer) {
            is FileAnswer.Accept -> answer.message
            is FileAnswer.Error -> return refused(transfer, answer)
            FileAnswer.Ack -> {
                fail(transfer, FailureReason.PEER_ERROR)
                return Outcome.Done
            }
        }
        transfer.accepted = true
        val start = accept.fileIndex
        val count = transfer.infos.size
        val valid = when {
            start < 0 || start > count -> false
            start == count -> accept.offset == 0L
            else -> accept.offset in 0..transfer.infos[start].size
        }
        if (!valid) {
            fail(transfer, FailureReason.PEER_INVALID)
            sendCancel(link, transfer.peer, transfer.key.id)
            return Outcome.Done
        }
        transfer.acked.set(transfer.infos.take(start).sumOf { it.size } + if (start < count) accept.offset else 0)
        progressed(transfer)
        try {
            coroutineScope {
                for (index in start until count) {
                    sendFile(transfer, link, window, index, if (index == start) accept.offset else 0, answers = this)
                }
            }
        } catch (e: PeerEndedException) {
            // The desktop ended the transfer on its side.
            fail(transfer, reasonFor(e.code))
            return Outcome.Done
        } catch (e: FileReadException) {
            fail(transfer, if (e.changed) FailureReason.CHANGED else FailureReason.UNREADABLE)
            sendCancel(link, transfer.peer, transfer.key.id)
            return Outcome.Done
        }
        finish(transfer, TransferState.Completed)
        return Outcome.Done
    }

    /** What to do when the desktop answers an offer with [error]. */
    private fun refused(transfer: Outgoing, error: FileAnswer.Error): Outcome {
        val retryAfter = error.message.retryAfterMs.toLong()
        return when (error.code) {
            // It has four transfers of this phone's running, so this one waits its turn.
            ErrorCode.ERROR_CODE_BUSY, ErrorCode.ERROR_CODE_UNAVAILABLE ->
                Outcome.Later(if (retryAfter > 0) retryAfter else RETRY_DELAY_MILLIS)
            ErrorCode.ERROR_CODE_INTERNAL -> Outcome.Failed
            // The user cancelled it on the desktop before this offer resumed it. A desktop that
            // cancelled it may also no longer know it.
            ErrorCode.ERROR_CODE_DECLINED -> {
                finish(transfer, TransferState.Cancelled(byPeer = true))
                Outcome.Done
            }
            ErrorCode.ERROR_CODE_NOT_FOUND -> {
                finish(transfer, if (transfer.accepted) TransferState.Cancelled(byPeer = true) else TransferState.Failed(FailureReason.PEER_ERROR))
                Outcome.Done
            }
            else -> {
                fail(transfer, reasonFor(error.code))
                Outcome.Done
            }
        }
    }

    /**
     * Sends file [index] of [transfer] from byte [from] on: the bytes before
     * it only go into the hash. Each chunk waits for room in [window]; its
     * answer is awaited in [answers].
     */
    private suspend fun sendFile(transfer: Outgoing, link: FileLink, window: Window, index: Int, from: Long, answers: CoroutineScope) {
        val size = transfer.infos[index].size
        val digest = sha256()
        val buffer = ByteArray(MAX_CHUNK_BYTES)
        val input = try {
            transfer.files[index].open()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            throw FileReadException(changed = false, e)
        }
        input.use { stream ->
            var skipped = 0L
            while (skipped < from) {
                val length = minOf(buffer.size.toLong(), from - skipped).toInt()
                read(stream, buffer, length)
                digest.update(buffer, 0, length)
                skipped += length
            }
            var offset = from
            do {
                val length = minOf(buffer.size.toLong(), size - offset).toInt()
                read(stream, buffer, length)
                digest.update(buffer, 0, length)
                val last = offset + length == size
                // A file that grew since it was offered would arrive cut short.
                if (last && readByte(stream) != -1) throw FileReadException(changed = true)
                val chunk = fileChunk {
                    transferId = transfer.key.id.bytes
                    fileIndex = index
                    this.offset = offset
                    data = ByteString.copyFrom(buffer, 0, length)
                    if (last) sha256 = ByteString.copyFrom(digest.digest())
                }
                window.places.acquire()
                // Once the chunk is queued it goes out even if the transfer stops, so it keeps its place until answered.
                val pending = try {
                    withContext(NonCancellable) { link.send(FileRequest.Chunk(chunk)) }
                } catch (e: Throwable) {
                    window.places.release()
                    throw e
                }
                val answer = window.answerOf(pending)
                answers.launch { answered(transfer, answer.await(), length) }
                offset += length
            } while (!last)
        }
    }

    private fun answered(transfer: Outgoing, answer: FileAnswer, length: Int) {
        transfer.answers.incrementAndGet()
        when (answer) {
            FileAnswer.Ack -> {
                transfer.acked.addAndGet(length.toLong())
                progressed(transfer)
            }
            is FileAnswer.Error -> throw PeerEndedException(answer.code)
            is FileAnswer.Accept -> throw PeerEndedException(ErrorCode.ERROR_CODE_INTERNAL)
        }
    }

    /** Reads exactly [length] bytes into [buffer]; a file that ends sooner has changed. */
    private fun read(stream: InputStream, buffer: ByteArray, length: Int) {
        var done = 0
        while (done < length) {
            val n = try {
                stream.read(buffer, done, length - done)
            } catch (e: Exception) {
                throw FileReadException(changed = false, e)
            }
            if (n < 0) throw FileReadException(changed = true)
            done += n
        }
    }

    private fun readByte(stream: InputStream): Int = try {
        stream.read()
    } catch (e: Exception) {
        throw FileReadException(changed = false, e)
    }

    /** The desktop cancelled the transfer [id]. */
    fun cancelledByPeer(link: FileLink, id: TransferId) {
        val transfer = synchronized(this) { transfers[TransferKey(Direction.OUTGOING, link.peer.id, id)] } ?: return
        if (finish(transfer, TransferState.Cancelled(byPeer = true))) transfer.job?.cancel()
    }

    /** The user cancelled [key]; the desktop is told if it may hold part of it. */
    fun cancel(key: TransferKey) {
        val transfer = synchronized(this) { transfers[key] } ?: return
        val link = synchronized(this) { transfer.link }
        if (!finish(transfer, TransferState.Cancelled(byPeer = false))) return
        transfer.job?.cancel()
        if (transfer.offered) sendCancel(link, transfer.peer, key.id)
    }

    fun cancelAll() {
        synchronized(this) { transfers.keys.toList() }.forEach(::cancel)
    }

    /**
     * Fails the transfers that waited longer than [KEEP_MILLIS] for a
     * session. The hour runs only while their desktop has no ready session
     * that would start them: one queued behind others while it is connected
     * keeps its place, but one that stopped on the only session there is
     * (stuck, or answered by a wedged desktop) does not.
     */
    fun sweep() {
        val now = clock()
        val expired = synchronized(this) {
            transfers.values.filter { !it.running && !it.state.isFinished }.filter { transfer ->
                if (readyLinks[transfer.peer.id].orEmpty().any { it !== transfer.stuckOn }) {
                    transfer.since = now
                    false
                } else {
                    now - transfer.since >= KEEP_MILLIS
                }
            }
        }
        expired.forEach { fail(it, FailureReason.EXPIRED) }
    }

    private fun setReady(link: FileLink, ready: Boolean) = synchronized(this) {
        val links = readyLinks.getOrPut(link.peer.id) { ArrayList() }
        if (ready) links += link else links.remove(link)
        if (links.isEmpty()) readyLinks.remove(link.peer.id)
    }

    private fun progressed(transfer: Outgoing) {
        synchronized(this) {
            if (transfer.state.isFinished) return
            transfer.since = clock()
            transfer.show()
        }
    }

    private fun setState(transfer: Outgoing, state: TransferState) {
        synchronized(this) {
            if (transfer.state.isFinished) return
            transfer.state = state
            transfer.show()
        }
    }

    private fun interrupt(transfer: Outgoing, stuckOn: FileLink?) {
        synchronized(this) {
            if (transfer.state.isFinished) return
            transfer.state = TransferState.Interrupted
            transfer.link = null
            transfer.stuckOn = stuckOn
            transfer.since = clock()
            transfer.show()
        }
    }

    private fun fail(transfer: Outgoing, reason: FailureReason) {
        finish(transfer, TransferState.Failed(reason))
    }

    /** Ends [transfer] in [state], unless it has ended already. Returns whether it did. */
    private fun finish(transfer: Outgoing, state: TransferState): Boolean {
        synchronized(this) {
            if (transfer.state.isFinished) return false
            transfer.state = state
            transfer.link = null
            transfers.remove(transfer.key)
            transfer.show()
        }
        return true
    }

    private fun poke() = wake.update { it + 1 }

    private fun reasonFor(code: ErrorCode): FailureReason = when (code) {
        ErrorCode.ERROR_CODE_DISABLED -> FailureReason.PEER_DISABLED
        ErrorCode.ERROR_CODE_PERMISSION_REQUIRED -> FailureReason.PEER_PERMISSION_REQUIRED
        ErrorCode.ERROR_CODE_NO_SPACE -> FailureReason.PEER_NO_SPACE
        else -> FailureReason.PEER_ERROR
    }

    internal companion object {
        /** How long to wait before offering again after a request went unanswered, or the desktop could not take it. */
        const val RETRY_DELAY_MILLIS = 5_000L

        /** Unanswered requests in a row after which the transfer stops for this session and waits for the next. */
        const val MAX_UNANSWERED = 3

        /** Offers answered `INTERNAL` that are tried again, waiting twice as long each time, before the transfer fails. */
        const val MAX_FAILURES = 3
    }
}
