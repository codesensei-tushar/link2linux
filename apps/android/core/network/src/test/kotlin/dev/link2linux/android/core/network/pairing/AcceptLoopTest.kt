package dev.link2linux.android.core.network.pairing

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException

class AcceptLoopTest {

    /** A listener whose first [failures] accepts fail, as when the app runs out of file descriptors. */
    private class FlakyServerSocket(private var failures: Int) : ServerSocket() {
        var attempts = 0
            private set

        override fun accept(): Socket {
            attempts++
            if (failures-- > 0) throw SocketException("Too many open files")
            return super.accept()
        }
    }

    @Test
    fun theLoopKeepsAcceptingAfterAFailedAccept() = runBlocking {
        val listener = FlakyServerSocket(failures = 3).apply { bind(InetSocketAddress(InetAddress.getLoopbackAddress(), 0)) }
        val accepted = CompletableDeferred<Socket>()
        val loop = launch(Dispatchers.IO) {
            acceptUntilClosed(listener, backoffMillis = 10, maxBackoffMillis = 20) { accepted.complete(it) }
        }
        try {
            val client = withContext(Dispatchers.IO) { Socket(InetAddress.getLoopbackAddress(), listener.localPort) }
            withTimeout(5_000) { accepted.await() }.close()
            client.close()
            // Three failures, then the accept that got the client; the loop may be in the next one already.
            assertTrue(listener.attempts >= 4)
        } finally {
            // accept() does not end on interruption, only when the socket closes.
            listener.close()
        }
        withTimeout(5_000) { loop.join() }
    }
}
