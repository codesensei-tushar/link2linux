package dev.link2linux.android.core.files

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FileAccept
import dev.link2linux.protocol.v1.fileCancel
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The phone sending files: every rule of "Sending" and "Resuming and cancelling", seen from the sender. */
@OptIn(ExperimentalCoroutinesApi::class)
class FileSenderTest {

    private val chunk = FileEngine.MAX_CHUNK_BYTES

    private fun sending(block: suspend Sending.() -> Unit) = runTest {
        val sending = Sending(this)
        try {
            sending.block()
        } finally {
            sending.h.close()
        }
    }

    private class Sending(val test: TestScope) {
        val h = Harness(test)
        val link = FakeLink()

        suspend fun send(vararg files: FakeFile, to: FilePeer = DESKTOP): TransferKey =
            h.engine.send(to, files.toList()).single().also { h.idle() }

        suspend fun attach(link: FakeLink = this.link) {
            h.engine.attach(link)
            h.idle()
        }

        suspend fun answer(sent: FakeLink.Sent, answer: FileAnswer) {
            sent.answer(answer)
            h.idle()
        }

        suspend fun accept(sent: FakeLink.Sent, fileIndex: Int = 0, offset: Long = 0) =
            answer(sent, FileAnswer.Accept(FileAccept.newBuilder().setFileIndex(fileIndex).setOffset(offset).build()))

        suspend fun refuse(sent: FakeLink.Sent, code: ErrorCode, retryAfterMillis: Int = 0) =
            answer(sent, failure(code, retryAfterMillis = retryAfterMillis))

        /** Acks every chunk as it comes, checking the window each time. */
        suspend fun ackAll(on: FakeLink = link) {
            while (true) {
                assertTrue("unanswered chunks", on.unanswered.count { it.request is FileRequest.Chunk } <= FileEngine.MAX_UNANSWERED_CHUNKS)
                val next = on.unanswered.firstOrNull { it.request is FileRequest.Chunk } ?: return
                answer(next, FileAnswer.Ack)
            }
        }

        /** The bytes of file [file] of [key] that went out on [on], in order. */
        fun received(key: TransferKey, file: Int, on: FakeLink = link): ByteArray =
            on.chunks.map { it.chunk }.filter { TransferId(it.transferId) == key.id && it.fileIndex == file }
                .fold(ByteArray(0)) { all, c -> all + c.data.toByteArray() }

        suspend fun advance(millis: Long) {
            test.testScheduler.advanceTimeBy(millis)
            test.testScheduler.runCurrent()
        }
    }

    @Test
    fun anOfferListsTheFilesWithSafeNames() = sending {
        val key = send(FakeFile("Report: Q3/final.pdf", bytes(10), mimeType = "application/pdf", modifiedUnixMs = 42), FakeFile("", bytes(0)))
        assertEquals(TransferState.Waiting, h.state(key))
        attach()
        val offer = link.offers.single().offer
        assertEquals(key.id, TransferId(offer.transferId))
        assertEquals(TransferId.SIZE, offer.transferId.size())
        assertEquals(h.now, offer.createdUnixMs)
        assertEquals(listOf("Report_ Q3_final.pdf", "file"), offer.filesList.map { it.name })
        assertEquals(listOf(10L, 0L), offer.filesList.map { it.size })
        assertEquals("application/pdf", offer.getFiles(0).mimeType)
        assertEquals(42L, offer.getFiles(0).modifiedUnixMs)
        assertEquals(TransferState.Running, h.state(key))
    }

    @Test
    fun chunksFollowInOrderAndTheLastOfEachFileCarriesItsHash() = sending {
        val big = bytes(2 * chunk + 1000)
        val small = bytes(5, seed = 9)
        val key = send(FakeFile("big", big), FakeFile("small", small), FakeFile("empty", ByteArray(0)))
        attach()
        accept(link.offers.single())
        ackAll()

        val chunks = link.chunks.map { it.chunk }
        assertEquals(listOf(0, 0, 0, 1, 2), chunks.map { it.fileIndex })
        assertEquals(listOf(0L, chunk.toLong(), 2L * chunk, 0L, 0L), chunks.map { it.offset })
        assertTrue(chunks.all { it.data.size() <= chunk })
        // Only the chunk that ends a file has a hash, and it covers the whole file.
        assertEquals(listOf(false, false, true, true, true), chunks.map { !it.sha256.isEmpty })
        assertArrayEquals(sha256Of(big), chunks[2].sha256.toByteArray())
        assertArrayEquals(sha256Of(small), chunks[3].sha256.toByteArray())
        // An empty file is one empty chunk with the hash of nothing.
        assertEquals(0, chunks[4].data.size())
        assertArrayEquals(sha256Of(ByteArray(0)), chunks[4].sha256.toByteArray())
        assertArrayEquals(big, received(key, 0))

        val done = h.transfer(key)
        assertEquals(TransferState.Completed, done.state)
        assertEquals(big.size + 5L, done.doneBytes)
        assertEquals(big.size + 5L, done.totalBytes)
    }

    @Test
    fun atMostEightChunksWaitForAnAnswerAcrossTransfers() = sending {
        val a = bytes(5 * chunk, seed = 1)
        val b = bytes(5 * chunk, seed = 2)
        val first = send(FakeFile("a", a))
        val second = send(FakeFile("b", b))
        attach()
        link.offers.forEach { accept(it) }
        assertEquals(FileEngine.MAX_UNANSWERED_CHUNKS, link.unanswered.size)
        assertEquals(8, link.chunks.size)
        ackAll()
        assertEquals(10, link.chunks.size)
        assertArrayEquals(a, received(first, 0))
        assertArrayEquals(b, received(second, 0))
        assertEquals(TransferState.Completed, h.state(first))
        assertEquals(TransferState.Completed, h.state(second))
    }

    @Test
    fun aResumedTransferHashesTheBytesTheDesktopHasAlready() = sending {
        val first = bytes(1000)
        val second = bytes(2 * chunk + 10, seed = 4)
        val key = send(FakeFile("first", first), FakeFile("second", second))
        attach()
        accept(link.offers.single(), fileIndex = 1, offset = 100_000)
        assertEquals(first.size + 100_000L, h.transfer(key).doneBytes)
        ackAll()
        val chunks = link.chunks.map { it.chunk }
        assertTrue(chunks.all { it.fileIndex == 1 })
        assertEquals(100_000L, chunks.first().offset)
        assertArrayEquals(second.copyOfRange(100_000, second.size), received(key, 1))
        assertArrayEquals(sha256Of(second), chunks.last().sha256.toByteArray())
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun aDesktopThatHasEveryFileCompletesTheTransfer() = sending {
        val key = send(FakeFile("a", bytes(10)), FakeFile("b", bytes(10)))
        attach()
        accept(link.offers.single(), fileIndex = 2)
        assertTrue(link.chunks.isEmpty())
        assertEquals(TransferState.Completed, h.state(key))
        assertEquals(20L, h.transfer(key).doneBytes)
    }

    @Test
    fun anAcceptThatPointsOutsideTheFilesEndsTheTransfer() = sending {
        attach()
        for ((fileIndex, offset) in listOf(0 to 11L, 3 to 0L, 2 to 1L, -1 to 0L)) {
            val key = send(FakeFile("a", bytes(10)), FakeFile("b", bytes(10)))
            accept(link.offers.last(), fileIndex, offset)
            assertEquals(TransferState.Failed(FailureReason.PEER_INVALID), h.state(key))
            assertEquals(key.id, link.cancels.last().cancelId)
        }
        assertTrue(link.chunks.isEmpty())
    }

    @Test
    fun aRefusedOfferEndsTheTransferWithTheDesktopsReason() = sending {
        attach()
        val cases = listOf(
            ErrorCode.ERROR_CODE_DISABLED to TransferState.Failed(FailureReason.PEER_DISABLED),
            ErrorCode.ERROR_CODE_PERMISSION_REQUIRED to TransferState.Failed(FailureReason.PEER_PERMISSION_REQUIRED),
            ErrorCode.ERROR_CODE_NO_SPACE to TransferState.Failed(FailureReason.PEER_NO_SPACE),
            ErrorCode.ERROR_CODE_INVALID_ARGUMENT to TransferState.Failed(FailureReason.PEER_ERROR),
            ErrorCode.ERROR_CODE_UNSUPPORTED to TransferState.Failed(FailureReason.PEER_ERROR),
            ErrorCode.ERROR_CODE_NOT_FOUND to TransferState.Failed(FailureReason.PEER_ERROR),
            ErrorCode.ERROR_CODE_DECLINED to TransferState.Cancelled(byPeer = true),
        )
        for ((code, state) in cases) {
            val key = send(FakeFile("a", bytes(10)))
            refuse(link.offers.last(), code)
            assertEquals(state, h.state(key))
        }
        assertTrue(link.chunks.isEmpty())
        assertTrue(link.cancels.isEmpty())
    }

    @Test
    fun aBusyDesktopIsOfferedTheTransferAgainLater() = sending {
        val key = send(FakeFile("a", bytes(10)))
        attach()
        // BUSY never uses up the retries.
        repeat(FileSender.MAX_UNANSWERED + 2) { n ->
            refuse(link.offers.last(), ErrorCode.ERROR_CODE_BUSY, retryAfterMillis = 7_000)
            assertEquals(TransferState.Interrupted, h.state(key))
            advance(6_999)
            assertEquals(n + 1, link.offers.size)
            advance(1)
            assertEquals(n + 2, link.offers.size)
        }
        accept(link.offers.last())
        ackAll()
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun anErrorOnAChunkEndsTheTransfer() = sending {
        val key = send(FakeFile("a", bytes(5 * chunk)))
        attach()
        accept(link.offers.single())
        refuse(link.chunks.first(), ErrorCode.ERROR_CODE_NO_SPACE)
        assertEquals(TransferState.Failed(FailureReason.PEER_NO_SPACE), h.state(key))
        // The desktop ended it on its side, so nothing more goes out.
        assertEquals(5, link.chunks.size)
        link.unanswered.forEach { it.answer(FileAnswer.Ack) }
        h.idle()
        assertEquals(5, link.chunks.size)
        assertTrue(link.cancels.isEmpty())
        assertEquals(TransferState.Failed(FailureReason.PEER_NO_SPACE), h.state(key))
    }

    @Test
    fun anUnansweredChunkMakesTheSenderOfferAgain() = sending {
        val data = bytes(3 * chunk)
        val key = send(FakeFile("a", data))
        attach()
        accept(link.offers.single())
        answer(link.chunks[0], FileAnswer.Ack)
        link.chunks[1].timeOut()
        h.idle()
        assertEquals(TransferState.Interrupted, h.state(key))
        advance(FileSender.RETRY_DELAY_MILLIS)
        assertEquals(2, link.offers.size)
        assertEquals(key.id, TransferId(link.offers[1].offer.transferId))
        // The desktop got the first chunk only.
        val before = link.chunks.size
        accept(link.offers[1], offset = chunk.toLong())
        ackAll()
        val resumed = link.chunks.drop(before).map { it.chunk }
        assertEquals(listOf(chunk.toLong(), 2L * chunk), resumed.map { it.offset })
        assertArrayEquals(sha256Of(data), resumed.last().sha256.toByteArray())
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun afterThreeUnansweredRequestsInARowTheTransferWaitsForTheNextSession() = sending {
        val key = send(FakeFile("a", bytes(10)))
        attach()
        repeat(FileSender.MAX_UNANSWERED) {
            link.offers.last().timeOut()
            h.idle()
            advance(FileSender.RETRY_DELAY_MILLIS)
        }
        assertEquals(FileSender.MAX_UNANSWERED, link.offers.size)
        assertEquals(TransferState.Interrupted, h.state(key))
        advance(10 * FileSender.RETRY_DELAY_MILLIS)
        assertEquals(FileSender.MAX_UNANSWERED, link.offers.size)

        val next = FakeLink()
        attach(next)
        accept(next.offers.single())
        ackAll(next)
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun anAnswerBreaksARowOfUnansweredRequests() = sending {
        val data = bytes(2 * chunk)
        val key = send(FakeFile("a", data))
        attach()
        link.offers[0].timeOut()
        h.idle()
        advance(FileSender.RETRY_DELAY_MILLIS)
        // Answered: the row starts again with the chunk that goes unanswered next.
        accept(link.offers[1])
        link.chunks.first().timeOut()
        h.idle()
        advance(FileSender.RETRY_DELAY_MILLIS)
        link.offers[2].timeOut()
        h.idle()
        advance(FileSender.RETRY_DELAY_MILLIS)
        assertEquals(4, link.offers.size)
        accept(link.offers[3])
        ackAll()
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun anOfferAnsweredInternalIsTriedAgainAFewTimesWithBackoff() = sending {
        val key = send(FakeFile("a", bytes(10)))
        attach()
        var wait = FileSender.RETRY_DELAY_MILLIS
        repeat(FileSender.MAX_FAILURES) { n ->
            refuse(link.offers.last(), ErrorCode.ERROR_CODE_INTERNAL)
            advance(wait - 1)
            assertEquals(n + 1, link.offers.size)
            advance(1)
            assertEquals(n + 2, link.offers.size)
            wait *= 2
        }
        refuse(link.offers.last(), ErrorCode.ERROR_CODE_INTERNAL)
        assertEquals(TransferState.Failed(FailureReason.PEER_ERROR), h.state(key))
    }

    @Test
    fun anUnavailableDesktopIsOfferedTheTransferAgainAfterFiveSeconds() = sending {
        send(FakeFile("a", bytes(10)))
        attach()
        refuse(link.offers.single(), ErrorCode.ERROR_CODE_UNAVAILABLE)
        advance(FileSender.RETRY_DELAY_MILLIS - 1)
        assertEquals(1, link.offers.size)
        advance(1)
        assertEquals(2, link.offers.size)
    }

    @Test
    fun aLostConnectionResumesWithTheSameTransferOnTheNextSession() = sending {
        val data = bytes(3 * chunk)
        val key = send(FakeFile("a", data))
        attach()
        accept(link.offers.single())
        answer(link.chunks[0], FileAnswer.Ack)
        link.close()
        h.idle()
        h.engine.detach(link)
        h.idle()
        assertEquals(TransferState.Interrupted, h.state(key))
        assertEquals(chunk.toLong(), h.transfer(key).doneBytes)

        val next = FakeLink()
        attach(next)
        val offer = next.offers.single().offer
        assertEquals(key.id, TransferId(offer.transferId))
        assertEquals(link.offers.single().offer.filesList, offer.filesList)
        accept(next.offers.single(), offset = chunk.toLong())
        ackAll(next)
        assertArrayEquals(data.copyOfRange(chunk, data.size), received(key, 0, on = next))
        assertArrayEquals(sha256Of(data), next.chunks.last().chunk.sha256.toByteArray())
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun offersWaitUntilFilesAreReadyOnTheDesktop() = sending {
        val notReady = FakeLink(ready = false)
        val key = send(FakeFile("a", bytes(5 * chunk)))
        attach(notReady)
        assertTrue(notReady.sent.isEmpty())
        notReady.peerReady.value = true
        h.idle()
        accept(notReady.offers.single())
        // The desktop turns files off midway: the transfer stops and waits.
        notReady.peerReady.value = false
        h.idle()
        assertEquals(TransferState.Interrupted, h.state(key))
        val sentBefore = notReady.chunks.size
        notReady.unanswered.forEach { it.answer(FileAnswer.Ack) }
        h.idle()
        assertEquals(sentBefore, notReady.chunks.size)
        notReady.peerReady.value = true
        h.idle()
        assertEquals(2, notReady.offers.size)
    }

    @Test
    fun nothingGoesOutWhileTheFeatureIsOffHere() = sending {
        h.policy.enabled = false
        h.engine.onPolicyChanged()
        val key = send(FakeFile("a", bytes(10)))
        attach()
        assertTrue(link.sent.isEmpty())
        h.policy.enabled = true
        h.engine.onPolicyChanged()
        h.idle()
        accept(link.offers.single())
        ackAll()
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun turningTheFeatureOffCancelsWhatIsGoingOut() = sending {
        val key = send(FakeFile("a", bytes(5 * chunk)))
        attach()
        accept(link.offers.single())
        h.policy.enabled = false
        h.engine.onPolicyChanged()
        h.idle()
        assertEquals(TransferState.Cancelled(byPeer = false), h.state(key))
        assertEquals(key.id, link.cancels.single().cancelId)
    }

    @Test
    fun theUserCancelsAndTheDesktopIsTold() = sending {
        val key = send(FakeFile("a", bytes(10 * chunk)))
        attach()
        accept(link.offers.single())
        answer(link.chunks[0], FileAnswer.Ack)
        h.engine.cancel(key)
        h.idle()
        assertEquals(TransferState.Cancelled(byPeer = false), h.state(key))
        assertEquals(key.id, link.cancels.single().cancelId)
        // Chunks that were on their way come back NOT_FOUND, which is ignored.
        val sent = link.chunks.size
        link.unanswered.filter { it.request is FileRequest.Chunk }.forEach { it.answer(failure(ErrorCode.ERROR_CODE_NOT_FOUND)) }
        h.idle()
        assertEquals(sent, link.chunks.size)
        assertEquals(TransferState.Cancelled(byPeer = false), h.state(key))
    }

    @Test
    fun aDesktopThatNoLongerKnowsAResumedTransferCancelledIt() = sending {
        val key = send(FakeFile("a", bytes(3 * chunk)))
        attach()
        accept(link.offers.single())
        link.close()
        h.idle()
        h.engine.detach(link)
        val next = FakeLink()
        attach(next)
        refuse(next.offers.single(), ErrorCode.ERROR_CODE_NOT_FOUND)
        assertEquals(TransferState.Cancelled(byPeer = true), h.state(key))
    }

    @Test
    fun aCancelWhileTheOfferWaitsForItsAnswerStillTellsTheDesktop() = sending {
        val key = send(FakeFile("a", bytes(10)))
        attach()
        assertFalse(link.offers.single().answered)
        h.engine.cancel(key)
        h.idle()
        assertEquals(key.id, link.cancels.single().cancelId)
    }

    @Test
    fun aTransferCancelledBeforeItWasOfferedNeedsNoCancel() = sending {
        val key = send(FakeFile("a", bytes(10)))
        h.engine.cancel(key)
        attach()
        assertTrue(link.sent.isEmpty())
        assertEquals(TransferState.Cancelled(byPeer = false), h.state(key))
    }

    @Test
    fun theDesktopCancels() = sending {
        val key = send(FakeFile("a", bytes(10 * chunk)))
        attach()
        accept(link.offers.single())
        val answer = h.engine.handle(link, FileRequest.Cancel(fileCancel { transferId = key.id.bytes }))
        h.idle()
        assertEquals(FileAnswer.Ack, answer)
        assertEquals(TransferState.Cancelled(byPeer = true), h.state(key))
        val sent = link.chunks.size
        link.unanswered.forEach { it.answer(FileAnswer.Ack) }
        h.idle()
        assertEquals(sent, link.chunks.size)
        assertTrue(link.cancels.isEmpty())
    }

    @Test
    fun aFileThatCannotBeReadIsNotSent() = sending {
        val file = FakeFile("a", bytes(10)).apply { openFails = true }
        val key = send(FakeFile("ok", bytes(10)), file)
        attach()
        accept(link.offers.single())
        ackAll()
        assertEquals(TransferState.Failed(FailureReason.UNREADABLE), h.state(key))
        assertEquals(key.id, link.cancels.single().cancelId)
    }

    @Test
    fun aFileThatChangedSizeIsNotSent() = sending {
        attach()
        // Shorter than when it was chosen, and longer.
        for (now in listOf(bytes(5), bytes(15))) {
            val file = FakeFile("a", bytes(10))
            val key = send(file)
            file.bytes = now
            accept(link.offers.last())
            assertEquals(TransferState.Failed(FailureReason.CHANGED), h.state(key))
            assertEquals(key.id, link.cancels.last().cancelId)
        }
        assertTrue(link.chunks.isEmpty())
    }

    @Test
    fun aTransferWaitsAnHourForItsDesktop() = sending {
        val waiting = send(FakeFile("a", bytes(10)))
        h.now += FileEngine.KEEP_MILLIS - 1
        h.engine.sweep()
        assertEquals(TransferState.Waiting, h.state(waiting))
        h.now += 1
        h.engine.sweep()
        assertEquals(TransferState.Failed(FailureReason.EXPIRED), h.state(waiting))

        // An interrupted one gets its hour from the moment it stopped.
        val interrupted = send(FakeFile("b", bytes(5 * chunk)))
        attach()
        accept(link.offers.single())
        h.now += 30 * 60 * 1000L
        link.close()
        h.idle()
        h.engine.detach(link)
        h.now += FileEngine.KEEP_MILLIS - 1
        h.engine.sweep()
        assertEquals(TransferState.Interrupted, h.state(interrupted))
        h.now += 1
        h.engine.sweep()
        assertEquals(TransferState.Failed(FailureReason.EXPIRED), h.state(interrupted))
    }

    @Test
    fun aQueuedTransferKeepsItsPlaceWhileItsDesktopIsConnected() = sending {
        val keys = (1..5).map { send(FakeFile("f$it", bytes(10))) }
        attach()
        assertEquals(4, link.offers.size)
        // The first four take longer than an hour; the fifth waits its turn.
        h.now += 2 * FileEngine.KEEP_MILLIS
        h.engine.sweep()
        assertEquals(TransferState.Waiting, h.state(keys[4]))
        // Once the desktop is gone, its hour starts.
        link.close()
        h.idle()
        h.engine.detach(link)
        h.idle()
        h.now += FileEngine.KEEP_MILLIS - 1
        h.engine.sweep()
        assertFalse(h.state(keys[4])!!.isFinished)
        h.now += 1
        h.engine.sweep()
        assertEquals(TransferState.Failed(FailureReason.EXPIRED), h.state(keys[4]))
    }

    @Test
    fun aBusyDesktopUsesNoneOfTheHour() = sending {
        val key = send(FakeFile("a", bytes(10)))
        attach()
        repeat(3) {
            refuse(link.offers.last(), ErrorCode.ERROR_CODE_BUSY, retryAfterMillis = 1_000)
            h.now += FileEngine.KEEP_MILLIS
            advance(1_000)
        }
        assertEquals(4, link.offers.size)
        accept(link.offers.last())
        ackAll()
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun atMostFourTransfersRunAtOnce() = sending {
        val keys = (1..5).map { send(FakeFile("f$it", bytes(10))) }
        attach()
        assertEquals(4, link.offers.size)
        assertEquals(TransferState.Waiting, h.state(keys[4]))
        accept(link.offers[0])
        ackAll()
        assertEquals(5, link.offers.size)
        assertEquals(keys[4].id, TransferId(link.offers[4].offer.transferId))
    }

    @Test
    fun manyFilesGoInSeveralTransfers() = sending {
        val keys = h.engine.send(DESKTOP, List(501) { FakeFile("f$it", ByteArray(0)) })
        assertEquals(2, keys.size)
        attach()
        assertEquals(listOf(500, 1), link.offers.map { it.offer.filesCount })
    }

    @Test
    fun transfersGoOnlyToTheirDesktop() = sending {
        send(FakeFile("a", bytes(10)), to = OTHER_DESKTOP)
        attach()
        assertTrue(link.sent.isEmpty())
        val other = FakeLink(OTHER_DESKTOP)
        attach(other)
        assertEquals(1, other.offers.size)
        assertFalse(link.sent.any())
    }
}
