package dev.link2linux.android.core.files

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.FileChunk
import dev.link2linux.protocol.v1.FileOffer
import dev.link2linux.protocol.v1.fileChunk
import dev.link2linux.protocol.v1.fileInfo
import dev.link2linux.protocol.v1.fileOffer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** Shared storage in memory. Names are made unique the way the phone's storage does it. */
class FakeStore : ReceivedFileStore {
    var free = Long.MAX_VALUE / 2
    var noSpaceOnAppend = false
    var failPublish = false

    /** While set, writing waits for it, as slow storage would; [appending] counts the writes that began. */
    @Volatile var gate: CountDownLatch? = null
    val appending = AtomicInteger()

    /** Published files, by the name they got. */
    val saved = LinkedHashMap<String, ByteArray>()

    /** Partial files that exist now, by their temporary name. */
    val partials = LinkedHashMap<String, Partial>()
    val leftovers = mutableListOf("leftover-1", "leftover-2")
    private var nextTemp = 0

    override fun freeBytes() = free

    override fun create(name: String, mimeType: String, size: Long): PartialFile {
        val partial = Partial(".partial-${nextTemp++}", name)
        partials[partial.temp] = partial
        return partial
    }

    override fun removeLeftovers() = leftovers.clear()

    inner class Partial(val temp: String, val name: String) : PartialFile {
        var bytes = ByteArray(0)

        override fun append(data: ByteArray) {
            appending.incrementAndGet()
            gate?.await(10, TimeUnit.SECONDS)
            if (noSpaceOnAppend) throw NoSpaceException("full")
            bytes += data
        }

        override fun publish(): SavedFile {
            partials.remove(temp)
            if (failPublish) throw IOException("publish failed")
            var unique = name
            var n = 0
            while (unique in saved) unique = FileNames.numbered(name, ++n)
            saved[unique] = bytes
            return SavedFile(unique, "content://downloads/$unique", "")
        }

        override fun delete() {
            partials.remove(temp)
        }
    }
}

class FakePolicy : FilePolicy {
    @Volatile override var enabled = true

    @Volatile override var storageGranted = true
}

/** A file to send. [bytes] may change after it was chosen, as a real file can. */
class FakeFile(
    override val name: String,
    var bytes: ByteArray,
    override val size: Long = bytes.size.toLong(),
    override val mimeType: String = "application/octet-stream",
    override val modifiedUnixMs: Long = 0,
) : OutgoingFile {
    var openFails = false
    var opens = 0

    override fun open(): InputStream {
        opens++
        if (openFails) throw SecurityException("no grant")
        return ByteArrayInputStream(bytes)
    }
}

/**
 * A session the test drives by hand: what the engine sends lands in [sent],
 * and the test answers each with [Sent.answer].
 */
class FakeLink(override val peer: FilePeer = DESKTOP, ready: Boolean = true) : FileLink {
    class Sent(val request: FileRequest) {
        private val reply = CompletableDeferred<FileAnswer>()
        val answered get() = reply.isCompleted

        fun answer(answer: FileAnswer) = reply.complete(answer)

        fun timeOut() = reply.completeExceptionally(AnswerTimeoutException("no answer"))

        fun drop() = reply.completeExceptionally(IOException("connection gone"))

        suspend fun await() = reply.await()

        val offer: FileOffer get() = (request as FileRequest.Offer).message
        val chunk: FileChunk get() = (request as FileRequest.Chunk).message
        val cancelId: TransferId get() = TransferId((request as FileRequest.Cancel).message.transferId)
    }

    override val peerReady = MutableStateFlow(ready)
    val sent = mutableListOf<Sent>()
    var closed = false

    override suspend fun send(request: FileRequest): PendingAnswer {
        if (closed) throw IOException("closed")
        val entry = Sent(request)
        sent += entry
        return PendingAnswer { entry.await() }
    }

    val unanswered get() = sent.filter { !it.answered }
    val chunks get() = sent.filter { it.request is FileRequest.Chunk }
    val offers get() = sent.filter { it.request is FileRequest.Offer }
    val cancels get() = sent.filter { it.request is FileRequest.Cancel }

    /** Closes the session: what waits for an answer fails, and nothing more can be sent. */
    fun close() {
        closed = true
        sent.filter { !it.answered }.forEach { it.drop() }
    }
}

/**
 * A session between two engines in one test: what one sends, the other
 * handles at once, in order.
 */
class LoopLink(override val peer: FilePeer, private val other: () -> FileEngine, private val otherSide: () -> FileLink) : FileLink {
    override val peerReady = MutableStateFlow(true)
    var closed = false
    val requests = mutableListOf<FileRequest>()

    /** When set, the request is not delivered and its answer never comes: the connection just died. */
    var cutAfter: Int = Int.MAX_VALUE

    override suspend fun send(request: FileRequest): PendingAnswer {
        if (closed || requests.size >= cutAfter) {
            closed = true
            throw IOException("closed")
        }
        requests += request
        val answer = other().handle(otherSide(), request)
        return PendingAnswer { answer }
    }
}

val DESKTOP = FilePeer("desktop-1", "Desktop")
val OTHER_DESKTOP = FilePeer("desktop-2", "Laptop")

/** An engine wired to fakes, on the test's scheduler. */
@OptIn(ExperimentalCoroutinesApi::class)
class Harness(val test: TestScope) {
    val store = FakeStore()
    val policy = FakePolicy()
    var now = 1_000_000_000L
    val errors = mutableListOf<Throwable>()
    private val scope = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(test.testScheduler))
    val engine = FileEngine(
        scope = scope,
        policy = policy,
        store = store,
        clock = { now },
        random = SecureRandom(),
        onError = { errors += it },
    )

    fun state(key: TransferKey): TransferState? = engine.transfers.value.firstOrNull { it.key == key }?.state

    fun transfer(key: TransferKey): Transfer = engine.transfers.value.first { it.key == key }

    val incoming get() = engine.transfers.value.filter { it.key.direction == Direction.INCOMING }

    /** Runs what is due now. Time moves only when a test advances it, so retry delays stay visible. */
    fun idle() = test.testScheduler.runCurrent()

    fun close() = scope.cancel()
}

fun id(n: Int): ByteString = ByteString.copyFrom(ByteArray(TransferId.SIZE) { n.toByte() })

fun offerOf(transfer: Int, vararg files: Pair<String, Long>): FileOffer = fileOffer {
    transferId = id(transfer)
    createdUnixMs = 1
    this.files += files.map { (name, size) ->
        fileInfo {
            this.name = name
            this.size = size
        }
    }
}

fun chunkOf(transfer: Int, file: Int, offset: Long, data: ByteArray, sha256: ByteArray? = null): FileChunk = fileChunk {
    transferId = id(transfer)
    fileIndex = file
    this.offset = offset
    this.data = ByteString.copyFrom(data)
    if (sha256 != null) this.sha256 = ByteString.copyFrom(sha256)
}

fun sha256Of(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

/** [size] bytes that differ from one position to the next. */
fun bytes(size: Int, seed: Int = 1): ByteArray = ByteArray(size) { (it * 31 + seed).toByte() }
