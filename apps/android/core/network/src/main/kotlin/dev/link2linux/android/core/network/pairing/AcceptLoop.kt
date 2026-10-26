package dev.link2linux.android.core.network.pairing

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.runInterruptible
import java.io.IOException
import java.net.ServerSocket
import java.net.Socket

/**
 * Accepts connections on [socket] until it is closed and hands each to
 * [onAccepted]. A failed accept while the socket is open, for example when
 * the app has run out of file descriptors, is tried again after a pause that
 * doubles from [backoffMillis] up to [maxBackoffMillis], so the listener
 * recovers once connections finish.
 */
internal suspend fun acceptUntilClosed(
    socket: ServerSocket,
    backoffMillis: Long = 100,
    maxBackoffMillis: Long = 2_000,
    onAccepted: (Socket) -> Unit,
) {
    var pause = backoffMillis
    while (!socket.isClosed) {
        val accepted = try {
            runInterruptible(Dispatchers.IO) { socket.accept() }
        } catch (e: IOException) {
            if (socket.isClosed) return
            delay(pause)
            pause = (pause * 2).coerceAtMost(maxBackoffMillis)
            continue
        }
        pause = backoffMillis
        onAccepted(accepted)
    }
}
