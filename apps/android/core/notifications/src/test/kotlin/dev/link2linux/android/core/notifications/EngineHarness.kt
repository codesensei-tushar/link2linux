package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.SyncCursor
import dev.link2linux.protocol.v1.notificationSyncRequest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher

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
        onError = { errors += it; it.printStackTrace() },
    )

    fun connect(vararg active: NotificationSnapshot) = engine.onListenerConnected(active.toList())

    fun post(s: NotificationSnapshot) = engine.onPosted(s)

    fun remove(key: String, cause: RemovalCause = RemovalCause.USER_DISMISSED) = engine.onRemoved(key, cause)

    suspend fun attach() {
        engine.attach(transport)
    }

    suspend fun sync(since: SyncCursor? = null): RecordingSink {
        val sink = RecordingSink()
        engine.handle(
            NotificationRequest.Sync(notificationSyncRequest { since?.let { this.since = it } }),
            sink,
        )
        return sink
    }

    /** Keys of all the notifications in the answer batches. */
    fun RecordingSink.keys() = replies.filterIsInstance<NotificationReply.Batch>()
        .flatMap { it.message.notificationsList }.map { it.key }

    fun RecordingSink.cursor(): SyncCursor = replies.filterIsInstance<NotificationReply.Batch>().last().message.cursor

    companion object {
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
