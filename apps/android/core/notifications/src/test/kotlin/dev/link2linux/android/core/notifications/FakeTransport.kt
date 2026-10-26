package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ErrorCode

/** A session stand-in: records the live events the engine sends. */
class FakeTransport : NotificationTransport {
    val events = mutableListOf<NotificationEvent>()
    var failWith: Exception? = null

    override suspend fun send(event: NotificationEvent) {
        failWith?.let { throw it }
        events += event
    }

    val posted get() = events.filterIsInstance<NotificationEvent.Posted>().map { it.message }
    val removed get() = events.filterIsInstance<NotificationEvent.Removed>().map { it.message }
}

class FakePolicy : NotificationPolicy {
    override var enabled = true
    override var accessGranted = true
    override var dismissSync = true
    override var hideOneTimeCodes = true
    val excluded = mutableSetOf<String>()
    val hidden = mutableSetOf<String>()

    override fun isExcluded(packageName: String) = packageName in excluded

    override fun isContentHidden(packageName: String) = packageName in hidden
}

class FakeEnvironment : NotificationEnvironment {
    override var isLocked = false
    override var canStartActivitiesFromBackground = false
}

class FakeCanceller : NotificationCanceller {
    val cancelled = mutableListOf<String>()
    var result = true

    override fun cancel(platformKey: String): Boolean {
        cancelled += platformKey
        return result
    }
}

/** Collects the answers to one request. */
class RecordingSink : ReplySink {
    val replies = mutableListOf<NotificationReply>()

    override suspend fun send(reply: NotificationReply) {
        replies += reply
    }

    val single get() = replies.single()
    val errorCode get() = (single as NotificationReply.Error).code

    fun assertError(code: ErrorCode) {
        org.junit.Assert.assertEquals(code, errorCode)
    }
}
