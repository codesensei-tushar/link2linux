package dev.link2linux.android.core.notifications

import com.google.protobuf.ByteString
import dev.link2linux.protocol.v1.NotificationActionRequest
import dev.link2linux.protocol.v1.NotificationDismissRequest
import dev.link2linux.protocol.v1.SyncCursor
import dev.link2linux.protocol.v1.imageRequest
import dev.link2linux.protocol.v1.notificationActionRequest
import dev.link2linux.protocol.v1.notificationDismissRequest
import dev.link2linux.protocol.v1.notificationSyncRequest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle

/** An engine wired to fakes. Its consumer runs eagerly on the test scheduler (background scopes do not run under advanceUntilIdle). */
@OptIn(ExperimentalCoroutinesApi::class)
class EngineHarness(val scope: TestScope, ownPackage: String = "dev.link2linux.android") {
    val policy = FakePolicy()
    val env = FakeEnvironment()
    val canceller = FakeCanceller()
    val images = ImageStore()
    val transport = FakeTransport()
    var now = 1_000_000_000L
    private var epochs = 0L
    val errors = mutableListOf<Throwable>()

    val engine = NotificationEngine(
        scope = CoroutineScope(SupervisorJob() + UnconfinedTestDispatcher(scope.testScheduler)),
        policy = policy,
        environment = env,
        mapper = NotificationMapper(images),
        images = images,
        canceller = canceller,
        ownPackage = ownPackage,
        clock = { now },
        newEpoch = { ++epochs },
        onError = { errors += it },
    )

    suspend fun idle() = scope.advanceUntilIdle()

    suspend fun connect(vararg active: NotificationSnapshot) {
        engine.onListenerConnected(active.toList())
        idle()
    }

    suspend fun post(s: NotificationSnapshot) {
        engine.onPosted(s)
        idle()
    }

    suspend fun remove(key: String, cause: RemovalCause = RemovalCause.USER_DISMISSED) {
        engine.onRemoved(key, cause)
        idle()
    }

    suspend fun changePolicy() {
        engine.onPolicyChanged()
        idle()
    }

    suspend fun attach(transport: FakeTransport = this.transport) {
        engine.attach(transport)
    }

    suspend fun sync(since: SyncCursor? = null, from: FakeTransport = transport): RecordingSink {
        val sink = RecordingSink()
        engine.handle(
            NotificationRequest.Sync(notificationSyncRequest { since?.let { this.since = it } }),
            sink,
            from,
        )
        return sink
    }

    /** Sync keys of all the notifications in the answer batches. */
    fun RecordingSink.keys() = replies.filterIsInstance<NotificationReply.Batch>()
        .flatMap { it.message.notificationsList }.map { it.key }

    fun RecordingSink.cursor(): SyncCursor = replies.filterIsInstance<NotificationReply.Batch>().last().message.cursor

    suspend fun dismiss(
        key: String,
        idem: ByteString = idempotency(1),
        build: NotificationDismissRequest.Builder.() -> Unit = {},
    ): RecordingSink {
        val sink = RecordingSink()
        val req = notificationDismissRequest {
            idempotencyKey = idem
            createdUnixMs = now
            this.key = key
        }.toBuilder().apply(build).build()
        engine.handle(NotificationRequest.Dismiss(req), sink, transport)
        return sink
    }

    suspend fun act(
        key: String,
        actionId: String,
        reply: String = "",
        idem: ByteString = idempotency(2),
        build: NotificationActionRequest.Builder.() -> Unit = {},
    ): RecordingSink {
        val sink = RecordingSink()
        val req = notificationActionRequest {
            idempotencyKey = idem
            createdUnixMs = now
            this.key = key
            this.actionId = actionId
            replyText = reply
        }.toBuilder().apply(build).build()
        engine.handle(NotificationRequest.Action(req), sink, transport)
        return sink
    }

    suspend fun image(hash: ByteString): RecordingSink {
        val sink = RecordingSink()
        engine.handle(NotificationRequest.Image(imageRequest { sha256 = hash }), sink, transport)
        return sink
    }

    companion object {
        fun idempotency(n: Int): ByteString = ByteString.copyFrom(ByteArray(16) { n.toByte() })

        fun snap(
            key: String,
            pkg: String = "org.chat",
            text: String = "hello",
            importance: Int = AndroidValues.IMPORTANCE_HIGH,
        ) = NotificationSnapshot(
            key = key,
            packageName = pkg,
            appName = pkg,
            postTimeMillis = 5,
            title = "T",
            text = text,
            importance = importance,
        )
    }
}
