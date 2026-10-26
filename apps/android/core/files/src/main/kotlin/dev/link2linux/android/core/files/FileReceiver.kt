package dev.link2linux.android.core.files

import dev.link2linux.android.core.files.FileEngine.Companion.KEEP_MILLIS
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_CHUNK_BYTES
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_FILES
import dev.link2linux.android.core.files.FileEngine.Companion.MAX_RUNNING_PER_PEER
import dev.link2linux.android.core.files.FileEngine.Companion.SPARE_BYTES
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FileChunk
import dev.link2linux.protocol.v1.FileInfo
import dev.link2linux.protocol.v1.FileOffer
import dev.link2linux.protocol.v1.fileAccept
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.security.MessageDigest

/**
 * The receiving half of the engine: answers the desktops' offers and
 * chunks, and writes the files through the [store].
 *
 * Each transfer has its own lock, held while one of its chunks is written,
 * so a slow write holds up no other transfer. The receiver's lock guards
 * the table of transfers and is taken before a transfer's lock, never
 * after.
 */
internal class FileReceiver(
    private val store: ReceivedFileStore,
    private val board: TransferBoard,
    private val clock: () -> Long,
    private val availability: () -> FileAvailability,
    /** Tells the desktop that the user cancelled; sends it later if [FileLink] is null or gone. */
    private val sendCancel: (FileLink?, FilePeer, TransferId) -> Unit,
    private val onError: (Throwable) -> Unit,
) {

    private inner class Incoming(
        val key: TransferKey,
        val peer: FilePeer,
        val files: List<FileInfo>,
        /** The names to save under, made safe; the store makes them unique. */
        val names: List<String>,
    ) {
        val lock = Mutex()
        val createdUnixMs = clock()
        private val totalBytes = files.sumOf { it.size }

        // Guarded by lock, once the transfer is in the table.
        var link: FileLink? = null
        @Volatile var state: TransferState = TransferState.Running
        var fileIndex = 0
        @Volatile var offset = 0L
        var partial: PartialFile? = null
        var digest: MessageDigest = sha256()
        @Volatile var finishedBytes = 0L
        val saved = ArrayList<SavedFile>()

        /** The last chunk or offer, or the end of the transfer; it is kept for [KEEP_MILLIS] after. */
        var lastActivity = createdUnixMs

        /** Bytes still to be written. Read without the transfer's lock, so possibly a chunk behind. */
        val remainingBytes: Long get() = totalBytes - finishedBytes - offset

        fun show() = board.put(
            Transfer(key, peer, names, totalBytes, finishedBytes + offset, state, saved.toList(), createdUnixMs),
        )

        /** Ends the transfer: deletes the file it has not finished and keeps the finished ones. */
        fun end(state: TransferState) {
            partial?.delete()
            partial = null
            this.state = state
            link = null
            lastActivity = clock()
            show()
        }
    }

    private val lock = Mutex()
    private val transfers = HashMap<TransferKey, Incoming>() // guarded by lock

    /** Transfers the user cancelled here, and when, so an offer that resumes one is declined. Guarded by lock. */
    private val declined = HashMap<TransferKey, Long>()

    /** The answer to any request while the feature cannot be used, or null while it can. */
    private fun refusal(): FileAnswer.Error? = when (availability()) {
        FileAvailability.READY -> null
        FileAvailability.DISABLED -> failure(ErrorCode.ERROR_CODE_DISABLED, "Files are turned off on the phone")
        FileAvailability.PERMISSION_REQUIRED -> failure(
            ErrorCode.ERROR_CODE_PERMISSION_REQUIRED,
            "The phone needs storage access",
            permission = FileEngine.STORAGE_PERMISSION,
        )
    }

    suspend fun offer(link: FileLink, offer: FileOffer): FileAnswer {
        refusal()?.let { return it }
        offerProblem(offer)?.let { return failure(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, it) }
        sweep()
        val key = TransferKey(Direction.INCOMING, link.peer.id, TransferId(offer.transferId))
        return lock.withLock {
            if (key in declined) {
                return@withLock failure(ErrorCode.ERROR_CODE_DECLINED, "The transfer was cancelled on the phone")
            }
            // Bytes that may still be written, after what the accepted transfers have yet to write.
            val free = runCatching { store.freeBytes() }.onFailure(onError).getOrDefault(Long.MAX_VALUE)
            val room = free - SPARE_BYTES - transfers.values.filter { !it.state.isFinished }.sumOf { it.remainingBytes }
            transfers[key]?.let { known ->
                val answer = known.lock.withLock { resume(known, link, offer, room) }
                if (known.state is TransferState.Failed) transfers.remove(key)
                return@withLock answer
            }
            val running = transfers.values.count { it.peer.id == link.peer.id && it.link != null && it.state == TransferState.Running }
            if (running >= MAX_RUNNING_PER_PEER) {
                return@withLock failure(ErrorCode.ERROR_CODE_BUSY, "$running transfers are running", retryAfterMillis = BUSY_RETRY_MILLIS)
            }
            val size = offer.filesList.sumOf { it.size }
            if (size > room) return@withLock failure(ErrorCode.ERROR_CODE_NO_SPACE, "$size bytes do not fit")
            val transfer = Incoming(key, link.peer, offer.filesList, offer.filesList.map { FileNames.safe(it.name) })
            transfer.link = link
            transfers[key] = transfer
            transfer.show()
            accept(0, 0)
        }
    }

    /**
     * An offer for a transfer the receiver holds: where its bytes resume. Its
     * own bytes still to come are part of [room] already, so it fits while
     * that is not negative.
     */
    private fun resume(transfer: Incoming, link: FileLink, offer: FileOffer, room: Long): FileAnswer {
        val same = offer.filesCount == transfer.files.size &&
            transfer.files.indices.all { offer.getFiles(it).name == transfer.files[it].name && offer.getFiles(it).size == transfer.files[it].size }
        if (!same) return failure(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, "The offer does not match the transfer it resumes")
        return when (transfer.state) {
            TransferState.Completed -> accept(transfer.files.size, 0)
            TransferState.Running, TransferState.Interrupted -> {
                if (room < 0) {
                    transfer.end(TransferState.Failed(FailureReason.NO_SPACE))
                    return failure(ErrorCode.ERROR_CODE_NO_SPACE, "The rest does not fit")
                }
                transfer.link = link
                transfer.state = TransferState.Running
                transfer.lastActivity = clock()
                transfer.show()
                accept(transfer.fileIndex, transfer.offset)
            }
            // Ended transfers leave the table; this one is on its way out.
            else -> failure(ErrorCode.ERROR_CODE_NOT_FOUND, "The transfer has ended")
        }
    }

    suspend fun chunk(link: FileLink, chunk: FileChunk): FileAnswer {
        // Turning the feature off ended the transfers; their later chunks learn why.
        refusal()?.let { return it }
        val key = TransferKey(Direction.INCOMING, link.peer.id, TransferId(chunk.transferId))
        val transfer = lock.withLock { transfers[key] } ?: return failure(ErrorCode.ERROR_CODE_NOT_FOUND, "Unknown transfer")
        val answer = transfer.lock.withLock { write(transfer, link, chunk) }
        if (transfer.state.let { it is TransferState.Cancelled || it is TransferState.Failed }) {
            lock.withLock { if (transfers[key] === transfer) transfers.remove(key) }
        }
        return answer
    }

    /** Checks [chunk] against where [transfer] stands and writes it. Holds the transfer's lock. */
    private fun write(transfer: Incoming, link: FileLink, chunk: FileChunk): FileAnswer {
        when {
            transfer.state == TransferState.Completed -> return failure(ErrorCode.ERROR_CODE_NOT_FOUND, "The transfer is complete")
            transfer.state.isFinished -> return failure(ErrorCode.ERROR_CODE_NOT_FOUND, "The transfer has ended")
            // After a reconnect the sender offers the transfer again before its chunks.
            transfer.link !== link -> return failure(ErrorCode.ERROR_CODE_NOT_FOUND, "The transfer was not offered in this session")
        }
        val size = chunk.data.size()
        val file = transfer.files.getOrNull(chunk.fileIndex)
        val ends = file != null && chunk.offset == transfer.offset && chunk.offset + size == file.size
        val problem = when {
            chunk.fileIndex != transfer.fileIndex || file == null -> "file_index ${chunk.fileIndex}, expected ${transfer.fileIndex}"
            chunk.offset != transfer.offset -> "offset ${chunk.offset}, expected ${transfer.offset}"
            size > MAX_CHUNK_BYTES -> "$size bytes of data"
            size > file.size - chunk.offset -> "The chunk runs past the end of the file"
            size == 0 && file.size != 0L -> "An empty chunk of a file that is not empty"
            ends && chunk.sha256.size() != SHA256_BYTES -> "The last chunk of a file has ${chunk.sha256.size()} bytes of SHA-256"
            !ends && !chunk.sha256.isEmpty -> "SHA-256 on a chunk that does not end the file"
            else -> null
        }
        if (problem != null) {
            transfer.end(TransferState.Failed(FailureReason.PEER_INVALID))
            return failure(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, problem)
        }
        checkNotNull(file)
        try {
            val partial = transfer.partial
                ?: store.create(transfer.names[transfer.fileIndex], file.mimeType, file.size).also { transfer.partial = it }
            if (size > 0) partial.append(chunk.data.toByteArray())
        } catch (e: NoSpaceException) {
            transfer.end(TransferState.Failed(FailureReason.NO_SPACE))
            return failure(ErrorCode.ERROR_CODE_NO_SPACE, "The phone's storage is full")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            onError(e)
            transfer.end(TransferState.Failed(FailureReason.WRITE_FAILED))
            return failure(ErrorCode.ERROR_CODE_INTERNAL, "The file could not be written")
        }
        chunk.data.asReadOnlyByteBufferList().forEach(transfer.digest::update)
        transfer.offset += size
        transfer.lastActivity = clock()
        if (ends) {
            if (!MessageDigest.isEqual(transfer.digest.digest(), chunk.sha256.toByteArray())) {
                transfer.end(TransferState.Failed(FailureReason.DAMAGED))
                return failure(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, "The file does not match its SHA-256")
            }
            val saved = try {
                checkNotNull(transfer.partial).publish()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                onError(e)
                transfer.end(TransferState.Failed(FailureReason.WRITE_FAILED))
                return failure(ErrorCode.ERROR_CODE_INTERNAL, "The file could not be saved")
            }
            transfer.saved += saved
            transfer.partial = null
            transfer.finishedBytes += file.size
            transfer.fileIndex++
            transfer.offset = 0
            transfer.digest = sha256()
            if (transfer.fileIndex == transfer.files.size) {
                // Kept a while longer, so an offer that resumes it learns it is complete.
                transfer.state = TransferState.Completed
                transfer.link = null
            }
        }
        transfer.show()
        return FileAnswer.Ack
    }

    /** The desktop cancelled the transfer [id]. Finished files stay; an unknown transfer is fine. */
    suspend fun cancelledByPeer(link: FileLink, id: TransferId) {
        val key = TransferKey(Direction.INCOMING, link.peer.id, id)
        val transfer = lock.withLock { transfers[key]?.also { if (!it.state.isFinished) transfers.remove(key) } } ?: return
        transfer.lock.withLock { if (!transfer.state.isFinished) transfer.end(TransferState.Cancelled(byPeer = true)) }
    }

    /** The user cancelled [key] on the phone; its desktop is told. */
    suspend fun cancel(key: TransferKey) {
        val transfer = lock.withLock {
            val known = transfers[key]?.takeIf { !it.state.isFinished } ?: return
            transfers.remove(key)
            declined[key] = clock()
            known
        }
        // Out of the table, nothing else would end it: finish even if the caller goes away
        // while a chunk of it is being written.
        withContext(NonCancellable) {
            val link = transfer.lock.withLock {
                if (transfer.state.isFinished) return@withContext
                transfer.link.also { transfer.end(TransferState.Cancelled(byPeer = false)) }
            }
            sendCancel(link, transfer.peer, key.id)
        }
    }

    suspend fun cancelAll() {
        lock.withLock { transfers.keys.toList() }.forEach { cancel(it) }
    }

    /** The session [link] ended: its transfers wait for the desktop to resume them. */
    suspend fun detach(link: FileLink) {
        lock.withLock { transfers.values.filter { it.link === link } }.forEach { transfer ->
            transfer.lock.withLock {
                if (transfer.link === link && transfer.state == TransferState.Running) {
                    transfer.link = null
                    transfer.state = TransferState.Interrupted
                    transfer.lastActivity = clock()
                    transfer.show()
                }
            }
        }
    }

    /** Ends transfers that saw nothing for [KEEP_MILLIS], and forgets complete ones as long after. */
    suspend fun sweep() {
        val now = clock()
        val expired = lock.withLock {
            declined.entries.removeAll { now - it.value >= KEEP_MILLIS }
            transfers.values.filter { now - it.lastActivity >= KEEP_MILLIS }.onEach { transfers.remove(it.key) }
        }
        expired.forEach { transfer ->
            transfer.lock.withLock { if (!transfer.state.isFinished) transfer.end(TransferState.Failed(FailureReason.EXPIRED)) }
        }
    }

    /** Why [offer] breaks the protocol's limits, or null. */
    private fun offerProblem(offer: FileOffer): String? {
        if (offer.transferId.size() != TransferId.SIZE) return "transfer_id has ${offer.transferId.size()} bytes"
        if (offer.filesCount !in 1..MAX_FILES) return "${offer.filesCount} files"
        var total = 0L
        offer.filesList.forEachIndexed { index, file ->
            if (!FileNames.isValid(file.name)) return "The name of file $index breaks the rules"
            // uint64 sizes past Long.MAX_VALUE arrive negative.
            if (file.size < 0) return "The size of file $index is out of range"
            total += file.size
            if (total < 0) return "The files' sizes add up past 2^63"
        }
        return null
    }

    private fun accept(fileIndex: Int, offset: Long) = FileAnswer.Accept(
        fileAccept {
            this.fileIndex = fileIndex
            this.offset = offset
        },
    )

    private companion object {
        const val SHA256_BYTES = 32

        /** What a desktop that is BUSY waits before it offers again. */
        const val BUSY_RETRY_MILLIS = 5_000
    }
}

internal fun sha256(): MessageDigest = MessageDigest.getInstance("SHA-256")
