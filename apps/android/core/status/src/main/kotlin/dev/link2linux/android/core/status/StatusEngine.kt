package dev.link2linux.android.core.status

import dev.link2linux.protocol.v1.PhoneStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.retryWhen
import kotlinx.coroutines.flow.shareIn
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The phone's side of the phone status feature (docs/protocol.md, "Phone
 * status"): the battery, signal and sound mode its status bar shows.
 *
 * [current] answers a desktop's `PhoneStatusRequest`, and [follow] then
 * keeps that desktop current with `PhoneStatusChanged`. The engine listens
 * to the platform only while a desktop asks or follows, and for
 * [keepListeningMillis] after the last one stops, so a desktop that
 * reconnects at once does not restart every listener. A status that left
 * the platform with a value the protocol does not allow goes out corrected:
 * an operator name shortened to [MAX_OPERATOR_NAME_BYTES] at a character
 * boundary, and a percent or level held to its range.
 *
 * A source that fails is started again after [retryMillis], and [onFailure]
 * hears of it; sharing never ends with it. A desktop that asks while the
 * source has nothing to tell within [answerMillis] gets an empty status,
 * which shows nothing, instead of no answer.
 */
class StatusEngine(
    scope: CoroutineScope,
    source: StatusSource,
    /** Whether the user has the feature on. */
    val enabled: StateFlow<Boolean>,
    keepListeningMillis: Long = KEEP_LISTENING_MILLIS,
    private val intervalMillis: Long = CHANGE_INTERVAL_MILLIS,
    private val retryMillis: Long = RETRY_MILLIS,
    private val answerMillis: Long = ANSWER_MILLIS,
    /** Called with each failure of the source; it must not log what the failure carries. */
    onFailure: (Throwable) -> Unit = {},
) {

    // The replay ends with the listeners: a status from before then may be stale.
    private val statuses: SharedFlow<PhoneStatus> = source.statuses()
        .retryWhen { cause, _ ->
            onFailure(cause)
            delay(retryMillis)
            true
        }
        .map(::checked)
        .distinctUntilChanged()
        .shareIn(scope, SharingStarted.WhileSubscribed(keepListeningMillis, replayExpirationMillis = 0), replay = 1)

    /** The phone's status now, for a `PhoneStatusReply`. */
    suspend fun current(): PhoneStatus =
        withTimeoutOrNull(answerMillis) { statuses.first() } ?: PhoneStatus.getDefaultInstance()

    /**
     * Keeps one desktop current, which has [sent], until cancelled or until
     * the user turns the feature off. [send] gets each new status, at most
     * one every [intervalMillis] counted from the last one the desktop got,
     * starting with [sent]. A change in the meantime replaces the one
     * waiting, so only the newest goes out, and nothing goes out when the
     * status ends up as the desktop has it. Throws what [send] throws.
     */
    suspend fun follow(sent: PhoneStatus, send: suspend (PhoneStatus) -> Unit) = coroutineScope {
        // Subscribed for as long as the desktop follows, so the listeners stay on between updates.
        val latest = statuses.stateIn(this)
        launch {
            var last = sent
            while (true) {
                delay(intervalMillis)
                val next = latest.first { it != last }
                send(next)
                last = next
            }
        }
        enabled.first { !it }
        coroutineContext.cancelChildren()
    }

    // Fields are uint32 on the wire, so a negative Int here would be a huge number there.
    private fun checked(status: PhoneStatus): PhoneStatus =
        status.toBuilder().apply {
            if (hasBattery()) battery = battery.toBuilder().setPercent(battery.percent.coerceIn(0, 100)).build()
            if (hasCellular()) {
                cellular = cellular.toBuilder()
                    .setLevel(cellular.level.coerceIn(0, MAX_SIGNAL_LEVEL))
                    .setOperatorName(truncateUtf8(cellular.operatorName, MAX_OPERATOR_NAME_BYTES))
                    .build()
            }
            if (hasWifi()) wifi = wifi.toBuilder().setLevel(wifi.level.coerceIn(0, MAX_SIGNAL_LEVEL)).build()
        }.build()

    companion object {
        /** The shortest time between two `PhoneStatusChanged` to one desktop (docs/protocol.md, "Phone status"). */
        const val CHANGE_INTERVAL_MILLIS: Long = 5_000

        const val KEEP_LISTENING_MILLIS: Long = 5_000

        const val RETRY_MILLIS: Long = 2_000

        /** Under the 30 seconds a desktop waits for the reply, and over a few retries. */
        const val ANSWER_MILLIS: Long = 10_000
    }
}
