package dev.link2linux.android.core.files

import dev.link2linux.protocol.v1.ErrorCode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch

/** Cancels on real threads, where a chunk being written holds the transfer up. */
class FileCancelTest {

    @Test
    fun aCancelWhoseCallerGoesAwayStillEndsTheTransfer() = runBlocking {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val store = FakeStore()
        val engine = FileEngine(scope, FakePolicy(), store)
        val link = FakeLink()
        val key = TransferKey(Direction.INCOMING, DESKTOP.id, TransferId(id(1)))
        try {
            engine.handle(link, FileRequest.Offer(offerOf(1, "a" to 10L)))
            val gate = CountDownLatch(1)
            store.gate = gate
            val writing = launch(Dispatchers.IO) { engine.handle(link, FileRequest.Chunk(chunkOf(1, 0, 0, bytes(5)))) }
            withTimeout(5_000) { while (store.appending.get() == 0) delay(10) }

            // The screen's cancel takes the transfer off the table, then waits for the write; the
            // screen goes away meanwhile.
            val caller = launch(Dispatchers.Default) { engine.cancel(key) }
            delay(300)
            caller.cancel()
            gate.countDown()
            writing.join()

            withTimeout(5_000) {
                engine.transfers.first { list -> list.any { it.key == key && it.state == TransferState.Cancelled(byPeer = false) } }
            }
            assertTrue(store.partials.isEmpty())
            // It was the cancel that ended it: an offer that would resume it is declined.
            val again = engine.handle(link, FileRequest.Offer(offerOf(1, "a" to 10L))) as FileAnswer.Error
            assertEquals(ErrorCode.ERROR_CODE_DECLINED, again.code)
        } finally {
            store.gate = null
            scope.cancel()
        }
    }
}
