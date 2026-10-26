package dev.link2linux.android.core.files

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.FileAccept
import dev.link2linux.protocol.v1.fileCancel
import dev.link2linux.protocol.v1.fileInfo
import dev.link2linux.protocol.v1.fileOffer
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The phone receiving files: every rule of "Receiving" and "Resuming and cancelling". */
class FileReceiverTest {

    private fun receiving(block: suspend Receiving.() -> Unit) = runTest {
        val receiving = Receiving(this)
        try {
            receiving.block()
        } finally {
            receiving.close()
        }
    }

    private class Receiving(test: TestScope) {
        val h = Harness(test)
        val link = FakeLink()
        val store get() = h.store

        suspend fun offer(offer: dev.link2linux.protocol.v1.FileOffer, on: FileLink = link) =
            h.engine.handle(on, FileRequest.Offer(offer))

        suspend fun chunk(chunk: dev.link2linux.protocol.v1.FileChunk, on: FileLink = link) =
            h.engine.handle(on, FileRequest.Chunk(chunk))

        suspend fun cancel(transfer: Int, on: FileLink = link) =
            h.engine.handle(on, FileRequest.Cancel(fileCancel { transferId = id(transfer) }))

        /** Sends all of [data] as file [file] of [transfer], in chunks of at most [chunkBytes]. */
        suspend fun sendFile(transfer: Int, file: Int, data: ByteArray, chunkBytes: Int = 1000, from: Int = 0, on: FileLink = link): FileAnswer {
            var offset = from
            var answer: FileAnswer = FileAnswer.Ack
            do {
                val end = minOf(data.size, offset + chunkBytes)
                val last = end == data.size
                answer = chunk(chunkOf(transfer, file, offset.toLong(), data.copyOfRange(offset, end), if (last) sha256Of(data) else null), on)
                if (answer != FileAnswer.Ack) return answer
                offset = end
            } while (!last)
            return answer
        }

        val key get() = TransferKey(Direction.INCOMING, DESKTOP.id, TransferId(id(1)))

        fun close() = h.close()
    }

    private fun accept(fileIndex: Int, offset: Long) = FileAnswer.Accept(
        FileAccept.newBuilder().setFileIndex(fileIndex).setOffset(offset).build(),
    )

    private fun FileAnswer.code(): ErrorCode = (this as FileAnswer.Error).code

    @Test
    fun aTransferArrivesWithEveryFileCheckedAgainstItsHash() = receiving {
        val photo = bytes(2500)
        val notes = bytes(10, seed = 7)
        assertEquals(accept(0, 0), offer(offerOf(1, "photo.jpg" to 2500L, "notes.txt" to 10L, "empty" to 0L)))
        assertEquals(FileAnswer.Ack, sendFile(1, 0, photo))
        // The finished file is saved before the next one starts.
        assertArrayEquals(photo, store.saved["photo.jpg"])
        assertEquals(TransferState.Running, h.state(key))
        assertEquals(FileAnswer.Ack, sendFile(1, 1, notes))
        // An empty file is one empty chunk with the hash of nothing.
        assertEquals(FileAnswer.Ack, chunk(chunkOf(1, 2, 0, ByteArray(0), sha256Of(ByteArray(0)))))

        assertEquals(listOf("photo.jpg", "notes.txt", "empty"), store.saved.keys.toList())
        assertArrayEquals(notes, store.saved["notes.txt"])
        assertEquals(0, store.saved.getValue("empty").size)
        assertTrue(store.partials.isEmpty())
        val done = h.transfer(key)
        assertEquals(TransferState.Completed, done.state)
        assertEquals(2510L, done.doneBytes)
        assertEquals(listOf("photo.jpg", "notes.txt", "empty"), done.saved.map { it.name })
    }

    @Test
    fun offersAreRefusedWhileTheFeatureIsOffOrLacksStorage() = receiving {
        h.policy.enabled = false
        h.engine.onPolicyChanged()
        assertEquals(ErrorCode.ERROR_CODE_DISABLED, offer(offerOf(1, "a" to 1L)).code())

        h.policy.enabled = true
        h.policy.storageGranted = false
        h.engine.onPolicyChanged()
        val refusal = offer(offerOf(1, "a" to 1L)) as FileAnswer.Error
        assertEquals(ErrorCode.ERROR_CODE_PERMISSION_REQUIRED, refusal.code)
        assertEquals("storage", refusal.message.permission)
        assertEquals(FileAvailability.PERMISSION_REQUIRED, h.engine.availability.value)

        h.policy.storageGranted = true
        h.engine.onPolicyChanged()
        assertEquals(accept(0, 0), offer(offerOf(1, "a" to 1L)))
    }

    @Test
    fun offersThatBreakTheLimitsAreInvalid() = receiving {
        suspend fun invalid(offer: dev.link2linux.protocol.v1.FileOffer) = offer(offer).code()
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, *Array(501) { "f$it" to 1L })))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, "../evil" to 1L)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, ".." to 1L)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, "" to 1L)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, "a\u0000b" to 1L)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, "x".repeat(256) to 1L)))
        // A uint64 size past 2^63, and sizes that add up past it.
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, "a" to -1L)))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(offerOf(1, "a" to Long.MAX_VALUE, "b" to 1L)))
        val shortId = fileOffer {
            transferId = ByteString.copyFrom(ByteArray(15))
            files += fileInfo { name = "a" }
        }
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, invalid(shortId))
        assertEquals(accept(0, 0), offer(offerOf(1, *Array(500) { "f$it" to 0L })))
    }

    @Test
    fun fiveRunningTransfersFromOneDesktopAreTooMany() = receiving {
        for (n in 1..4) assertEquals(accept(0, 0), offer(offerOf(n, "f" to 10L)))
        val busy = offer(offerOf(5, "f" to 10L)) as FileAnswer.Error
        assertEquals(ErrorCode.ERROR_CODE_BUSY, busy.code)
        assertTrue(busy.message.retryAfterMs > 0)
        // Another desktop has a count of its own.
        assertEquals(accept(0, 0), offer(offerOf(5, "f" to 10L), on = FakeLink(OTHER_DESKTOP)))
        // A finished transfer no longer counts.
        assertEquals(FileAnswer.Ack, sendFile(1, 0, bytes(10)))
        assertEquals(accept(0, 0), offer(offerOf(5, "f" to 10L)))
    }

    @Test
    fun filesThatDoNotFitWithRoomToSpareAreRefused() = receiving {
        store.free = FileEngine.SPARE_BYTES + 1000
        assertEquals(ErrorCode.ERROR_CODE_NO_SPACE, offer(offerOf(1, "a" to 600L, "b" to 401L)).code())
        assertEquals(accept(0, 0), offer(offerOf(1, "a" to 600L, "b" to 400L)))
        assertTrue(h.incoming.size == 1)
    }

    @Test
    fun bytesStillToComeForAcceptedTransfersCount() = receiving {
        store.free = FileEngine.SPARE_BYTES + 1000
        assertEquals(accept(0, 0), offer(offerOf(1, "a" to 600L)))
        assertEquals(ErrorCode.ERROR_CODE_NO_SPACE, offer(offerOf(2, "b" to 401L)).code())
        // As the first arrives, less of it is still to come; free space is measured again each time.
        chunk(chunkOf(1, 0, 0, bytes(200)))
        store.free -= 200
        assertEquals(ErrorCode.ERROR_CODE_NO_SPACE, offer(offerOf(2, "b" to 401L)).code())
        assertEquals(accept(0, 0), offer(offerOf(2, "b" to 400L)))
    }

    @Test
    fun storageThatFillsUpEndsTheTransfer() = receiving {
        offer(offerOf(1, "a" to 10L))
        store.noSpaceOnAppend = true
        assertEquals(ErrorCode.ERROR_CODE_NO_SPACE, chunk(chunkOf(1, 0, 0, bytes(5))).code())
        assertEquals(TransferState.Failed(FailureReason.NO_SPACE), h.state(key))
        assertTrue(store.partials.isEmpty())
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 5, bytes(5))).code())
    }

    @Test
    fun aChunkOfAnUnknownTransferIsNotFound() = receiving {
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(9, 0, 0, bytes(5))).code())
        // Transfers are the desktop's own: another desktop's id is unknown here.
        offer(offerOf(1, "a" to 10L), on = FakeLink(OTHER_DESKTOP))
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 0, bytes(5))).code())
    }

    @Test
    fun chunksMustContinueWhereTheReceiverStands() = receiving {
        val data = bytes(3000)
        // Each case gets a fresh transfer, since a broken chunk ends the one it belongs to.
        val cases = listOf(
            chunkOf(1, 0, 1, data.copyOfRange(1, 100)), // skips a byte
            chunkOf(1, 1, 0, data.copyOfRange(0, 100)), // the next file first
            chunkOf(1, 0, 0, data.copyOfRange(0, 100), sha256Of(data)), // a hash before the end
            chunkOf(1, 0, 0, data, null), // the whole file without its hash
            chunkOf(1, 0, 0, data, ByteArray(16)), // a hash that is not SHA-256
            chunkOf(1, 0, 0, data + 1.toByte(), sha256Of(data + 1.toByte())), // past the end of the file
            chunkOf(1, 0, 0, ByteArray(0)), // empty, in a file that is not
        )
        cases.forEachIndexed { index, bad ->
            val desktop = FakeLink(FilePeer("desktop-$index", "Desktop $index"))
            assertEquals(accept(0, 0), offer(offerOf(1, "a" to 3000L, "b" to 5L), on = desktop))
            assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, chunk(bad, on = desktop).code())
            val ended = TransferKey(Direction.INCOMING, desktop.peer.id, TransferId(id(1)))
            assertEquals(TransferState.Failed(FailureReason.PEER_INVALID), h.state(ended))
            // It ended: what follows is unknown.
            assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 0, data.copyOfRange(0, 10)), on = desktop).code())
        }
        assertTrue(store.partials.isEmpty())
        assertTrue(store.saved.isEmpty())
    }

    @Test
    fun aChunkMayCarryAtMost256KiB() = receiving {
        val size = FileEngine.MAX_CHUNK_BYTES + 1
        offer(offerOf(1, "big" to size.toLong()))
        val tooBig = bytes(size)
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, chunk(chunkOf(1, 0, 0, tooBig, sha256Of(tooBig))).code())
    }

    @Test
    fun aFileThatDoesNotMatchItsHashIsDeleted() = receiving {
        val good = bytes(100)
        offer(offerOf(1, "good" to 100L, "bad" to 100L))
        sendFile(1, 0, good)
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, chunk(chunkOf(1, 1, 0, bytes(100, seed = 2), sha256Of(good))).code())
        assertEquals(TransferState.Failed(FailureReason.DAMAGED), h.state(key))
        // The file that matched stays; the other is gone.
        assertEquals(listOf("good"), store.saved.keys.toList())
        assertTrue(store.partials.isEmpty())
    }

    @Test
    fun receivedFilesNeverReplaceOthersAndKeepNoPath() = receiving {
        store.saved["photo.jpg"] = byteArrayOf(9)
        offer(offerOf(1, "photo.jpg" to 3L, "a\\b:c.txt" to 1L, ".profile" to 1L))
        sendFile(1, 0, bytes(3))
        sendFile(1, 1, bytes(1))
        sendFile(1, 2, bytes(1))
        assertArrayEquals(byteArrayOf(9), store.saved["photo.jpg"])
        assertEquals(listOf("photo.jpg", "photo (1).jpg", "a_b_c.txt", "_profile"), store.saved.keys.toList())
        assertEquals(listOf("photo (1).jpg", "a_b_c.txt", "_profile"), h.transfer(key).saved.map { it.name })
    }

    @Test
    fun anInterruptedTransferResumesWhereItStopped() = receiving {
        val data = bytes(5000)
        val first = bytes(300, seed = 3)
        offer(offerOf(1, "first" to 300L, "second" to 5000L))
        sendFile(1, 0, first)
        chunk(chunkOf(1, 1, 0, data.copyOfRange(0, 2000)))
        h.engine.detach(link)
        assertEquals(TransferState.Interrupted, h.state(key))

        // The desktop is back: the same offer learns where to go on.
        val again = FakeLink()
        assertEquals(accept(1, 2000), offer(offerOf(1, "first" to 300L, "second" to 5000L), on = again))
        assertEquals(TransferState.Running, h.state(key))
        assertEquals(FileAnswer.Ack, sendFile(1, 1, data, from = 2000, chunkBytes = 1500, on = again))
        assertArrayEquals(data, store.saved["second"])
        assertEquals(TransferState.Completed, h.state(key))

        // Offered once more, it is already complete.
        assertEquals(accept(2, 0), offer(offerOf(1, "first" to 300L, "second" to 5000L), on = again))
    }

    @Test
    fun chunksComeOnlyForATransferOfferedInTheirSession() = receiving {
        offer(offerOf(1, "a" to 10L))
        assertEquals(FileAnswer.Ack, chunk(chunkOf(1, 0, 0, bytes(10).copyOfRange(0, 4))))
        h.engine.detach(link)
        val again = FakeLink()
        // Neither the new session before it offers the transfer again, nor the old one.
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 4, bytes(10).copyOfRange(4, 10), sha256Of(bytes(10))), on = again).code())
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 4, bytes(10).copyOfRange(4, 10), sha256Of(bytes(10)))).code())
        assertEquals(TransferState.Interrupted, h.state(key))
        assertEquals(accept(0, 4), offer(offerOf(1, "a" to 10L), on = again))
        assertEquals(FileAnswer.Ack, chunk(chunkOf(1, 0, 4, bytes(10).copyOfRange(4, 10), sha256Of(bytes(10))), on = again))
        assertArrayEquals(bytes(10), store.saved["a"])
    }

    @Test
    fun aChunkOfACompleteTransferIsNotFound() = receiving {
        offer(offerOf(1, "a" to 5L))
        sendFile(1, 0, bytes(5))
        assertEquals(TransferState.Completed, h.state(key))
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 0, bytes(5), sha256Of(bytes(5)))).code())
        assertEquals(TransferState.Completed, h.state(key))
    }

    @Test
    fun aResumeThatNoLongerFitsIsRefused() = receiving {
        store.free = FileEngine.SPARE_BYTES + 1000
        offer(offerOf(1, "a" to 600L))
        chunk(chunkOf(1, 0, 0, bytes(100)))
        store.free -= 100
        h.engine.detach(link)
        // Something else filled the storage meanwhile.
        store.free -= 450
        assertEquals(ErrorCode.ERROR_CODE_NO_SPACE, offer(offerOf(1, "a" to 600L)).code())
        assertEquals(TransferState.Failed(FailureReason.NO_SPACE), h.state(key))
        assertTrue(store.partials.isEmpty())
        // It is gone: offered again, it starts afresh, and still does not fit.
        assertEquals(ErrorCode.ERROR_CODE_NO_SPACE, offer(offerOf(1, "a" to 600L)).code())
    }

    @Test
    fun aResumingOfferMustMatchTheTransfer() = receiving {
        offer(offerOf(1, "a" to 10L, "b" to 10L))
        h.engine.detach(link)
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, offer(offerOf(1, "a" to 10L)).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, offer(offerOf(1, "a" to 10L, "b" to 11L)).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, offer(offerOf(1, "b" to 10L, "a" to 10L)).code())
        assertEquals(accept(0, 0), offer(offerOf(1, "a" to 10L, "b" to 10L)))
    }

    @Test
    fun theDesktopCancelsAndFinishedFilesStay() = receiving {
        offer(offerOf(1, "a" to 10L, "b" to 10L))
        sendFile(1, 0, bytes(10))
        chunk(chunkOf(1, 1, 0, bytes(5)))
        assertEquals(1, store.partials.size)
        assertEquals(FileAnswer.Ack, cancel(1))
        assertEquals(TransferState.Cancelled(byPeer = true), h.state(key))
        assertEquals(listOf("a"), store.saved.keys.toList())
        assertTrue(store.partials.isEmpty())
        // Chunks already on their way are not found, and a transfer nobody knows is cancelled fine.
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 1, 5, bytes(5))).code())
        assertEquals(FileAnswer.Ack, cancel(7))
    }

    @Test
    fun theUserCancelsAndTheDesktopIsTold() = receiving {
        h.engine.attach(link)
        offer(offerOf(1, "a" to 10L))
        chunk(chunkOf(1, 0, 0, bytes(5)))
        h.engine.cancel(key)
        h.idle()
        assertEquals(TransferState.Cancelled(byPeer = false), h.state(key))
        assertTrue(store.partials.isEmpty())
        assertEquals(TransferId(id(1)), link.cancels.single().cancelId)
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, chunk(chunkOf(1, 0, 5, bytes(5))).code())
        // An offer that would resume it is declined.
        assertEquals(ErrorCode.ERROR_CODE_DECLINED, offer(offerOf(1, "a" to 10L)).code())
    }

    @Test
    fun aCancelTheDesktopMissedGoesOutWhenItIsBack() = receiving {
        offer(offerOf(1, "a" to 10L))
        h.engine.detach(link)
        h.engine.cancel(key)
        h.idle()
        assertTrue(link.cancels.isEmpty())
        val again = FakeLink(ready = false)
        h.engine.attach(again)
        h.idle()
        // Only once files are ready on the desktop.
        assertTrue(again.cancels.isEmpty())
        again.peerReady.value = true
        h.idle()
        assertEquals(TransferId(id(1)), again.cancels.single().cancelId)
    }

    @Test
    fun aCancelWaitingForAnEndedSessionGoesOutOnTheNext() = receiving {
        val notReady = FakeLink(ready = false)
        h.engine.attach(notReady)
        offer(offerOf(1, "a" to 10L), on = notReady)
        h.engine.cancel(key)
        h.idle()
        assertTrue(notReady.cancels.isEmpty())
        h.engine.detach(notReady)
        h.idle()
        val next = FakeLink()
        h.engine.attach(next)
        h.idle()
        assertEquals(TransferId(id(1)), next.cancels.single().cancelId)
        assertTrue(notReady.cancels.isEmpty())
    }

    @Test
    fun partialFilesAreKeptForAnHour() = receiving {
        offer(offerOf(1, "a" to 10L))
        chunk(chunkOf(1, 0, 0, bytes(5)))
        h.engine.detach(link)
        h.now += FileEngine.KEEP_MILLIS - 1
        h.engine.sweep()
        assertEquals(TransferState.Interrupted, h.state(key))
        assertEquals(1, store.partials.size)
        h.now += 1
        h.engine.sweep()
        assertEquals(TransferState.Failed(FailureReason.EXPIRED), h.state(key))
        assertTrue(store.partials.isEmpty())
        // Once forgotten, the same offer starts afresh.
        assertEquals(accept(0, 0), offer(offerOf(1, "a" to 10L)))
    }

    @Test
    fun leftoversOfAnEarlierRunGoBeforeTheFirstOffer() = receiving {
        h.idle()
        assertTrue(store.leftovers.isEmpty())
    }

    @Test
    fun turningTheFeatureOffStopsWhatIsArriving() = receiving {
        h.engine.attach(link)
        offer(offerOf(1, "a" to 10L))
        chunk(chunkOf(1, 0, 0, bytes(5)))
        h.policy.enabled = false
        h.engine.onPolicyChanged()
        h.idle()
        assertEquals(TransferState.Cancelled(byPeer = false), h.state(key))
        assertTrue(store.partials.isEmpty())
        assertEquals(1, link.cancels.size)
        // Chunks still on their way learn why.
        assertEquals(ErrorCode.ERROR_CODE_DISABLED, chunk(chunkOf(1, 0, 5, bytes(5))).code())
    }

    @Test
    fun aFileThatCannotBeSavedEndsTheTransfer() = receiving {
        offer(offerOf(1, "a" to 5L))
        store.failPublish = true
        assertEquals(ErrorCode.ERROR_CODE_INTERNAL, sendFile(1, 0, bytes(5)).code())
        assertEquals(TransferState.Failed(FailureReason.WRITE_FAILED), h.state(key))
    }

    @Test
    fun finishedTransfersCanBeDismissed() = receiving {
        offer(offerOf(1, "a" to 1L))
        h.engine.dismiss(key)
        // Still running: it stays.
        assertEquals(1, h.incoming.size)
        sendFile(1, 0, bytes(1))
        h.engine.dismiss(key)
        assertTrue(h.incoming.isEmpty())
    }
}
