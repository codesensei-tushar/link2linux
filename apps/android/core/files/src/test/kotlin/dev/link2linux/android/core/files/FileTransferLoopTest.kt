package dev.link2linux.android.core.files

import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Two engines talking to each other: what one sends, the other saves, also across a lost connection. */
class FileTransferLoopTest {

    private val chunk = FileEngine.MAX_CHUNK_BYTES
    private val phone = FilePeer("phone", "Phone")

    /** A session between [sender] and [receiver], seen from both ends. */
    private class Session(sender: Harness, receiver: Harness, senderPeer: FilePeer, receiverPeer: FilePeer) {
        lateinit var toReceiver: LoopLink
        lateinit var toSender: LoopLink

        init {
            toReceiver = LoopLink(receiverPeer, { receiver.engine }, { toSender })
            toSender = LoopLink(senderPeer, { sender.engine }, { toReceiver })
        }
    }

    @Test
    fun filesArriveWholeAndResumeAfterTheConnectionDrops() = runTest {
        val sender = Harness(this)
        val receiver = Harness(this)
        try {
            val photo = bytes(2 * chunk + 123, seed = 5)
            val notes = bytes(77, seed = 6)
            val first = Session(sender, receiver, phone, DESKTOP)
            // The connection dies after the offer and one chunk.
            first.toReceiver.cutAfter = 2
            receiver.engine.attach(first.toSender)
            sender.engine.attach(first.toReceiver)
            val key = sender.engine.send(DESKTOP, listOf(FakeFile("photo.jpg", photo), FakeFile("notes.txt", notes))).single()
            sender.idle()
            assertEquals(TransferState.Interrupted, sender.state(key))
            val incoming = TransferKey(Direction.INCOMING, phone.id, key.id)
            assertEquals(chunk.toLong(), receiver.transfer(incoming).doneBytes)

            sender.engine.detach(first.toReceiver)
            receiver.engine.detach(first.toSender)
            receiver.idle()
            assertEquals(TransferState.Interrupted, receiver.state(incoming))

            val second = Session(sender, receiver, phone, DESKTOP)
            receiver.engine.attach(second.toSender)
            sender.engine.attach(second.toReceiver)
            sender.idle()

            assertEquals(TransferState.Completed, sender.state(key))
            assertEquals(TransferState.Completed, receiver.state(incoming))
            assertArrayEquals(photo, receiver.store.saved["photo.jpg"])
            assertArrayEquals(notes, receiver.store.saved["notes.txt"])
            assertTrue(receiver.store.partials.isEmpty())
            // The second session started where the first stopped.
            assertEquals(chunk.toLong(), (second.toReceiver.requests[1] as FileRequest.Chunk).message.offset)
        } finally {
            sender.close()
            receiver.close()
        }
    }

    @Test
    fun aCancelReachesTheOtherSide() = runTest {
        val sender = Harness(this)
        val receiver = Harness(this)
        try {
            val link = Session(sender, receiver, phone, DESKTOP)
            link.toReceiver.cutAfter = 3
            receiver.engine.attach(link.toSender)
            sender.engine.attach(link.toReceiver)
            val key = sender.engine.send(DESKTOP, listOf(FakeFile("big", bytes(5 * chunk)))).single()
            sender.idle()
            // The receiver's user cancels while the connection is down; the sender hears it when it is back.
            sender.engine.detach(link.toReceiver)
            receiver.engine.detach(link.toSender)
            val incoming = TransferKey(Direction.INCOMING, phone.id, key.id)
            receiver.engine.cancel(incoming)
            receiver.idle()
            assertTrue(receiver.store.partials.isEmpty())

            val again = Session(sender, receiver, phone, DESKTOP)
            receiver.engine.attach(again.toSender)
            sender.engine.attach(again.toReceiver)
            sender.idle()
            receiver.idle()
            assertEquals(TransferState.Cancelled(byPeer = true), sender.state(key))
            assertTrue(receiver.store.saved.isEmpty())
        } finally {
            sender.close()
            receiver.close()
        }
    }
}
