package dev.link2linux.android.core.network.features

import dev.link2linux.android.core.model.Device
import dev.link2linux.android.core.model.DeviceId
import dev.link2linux.android.core.model.DeviceKind
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.core.model.KeyFingerprint
import dev.link2linux.android.core.model.PairedDevice
import dev.link2linux.android.core.model.PresenceKey
import dev.link2linux.android.core.network.Session
import dev.link2linux.android.core.network.Transport
import dev.link2linux.android.core.network.pairing.SessionHooks
import dev.link2linux.protocol.v1.Envelope
import dev.link2linux.protocol.v1.FeatureAvailability
import dev.link2linux.protocol.v1.FeatureStatus
import dev.link2linux.protocol.v1.envelope
import dev.link2linux.protocol.v1.featureState
import dev.link2linux.protocol.v1.featureStatus
import dev.link2linux.protocol.v1.ping
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream

class CompositeSessionHooksTest {

    /** One feature's hooks, whose state the test changes and whose calls it records. */
    private class FakeHooks(
        private val feature: Feature,
        @Volatile var availability: FeatureAvailability,
        private val takes: Set<Envelope.PayloadCase> = emptySet(),
    ) : SessionHooks {
        val changes = MutableSharedFlow<FeatureStatus>(extraBufferCapacity = 16)
        val messages = mutableListOf<Envelope.PayloadCase>()
        val ended = mutableListOf<Session>()
        var failOnEnd = false

        override fun features() = setOf(feature)

        override fun featureStatus() = status(feature to availability)

        override fun featureStatusChanges(): Flow<FeatureStatus> = changes

        override suspend fun onMessage(desktop: PairedDevice, session: Session, envelope: Envelope): Boolean {
            messages += envelope.payloadCase
            return envelope.payloadCase in takes
        }

        override suspend fun onEnded(desktop: PairedDevice, session: Session) {
            ended += session
            if (failOnEnd) throw IllegalStateException("$feature failed")
        }

        fun change(to: FeatureAvailability) {
            availability = to
            check(changes.tryEmit(featureStatus()))
        }
    }

    private val notifications = FakeHooks(
        Feature.NOTIFICATIONS,
        FeatureAvailability.FEATURE_AVAILABILITY_READY,
        takes = setOf(Envelope.PayloadCase.NOTIFICATION_SYNC_REQUEST),
    )
    private val clipboard = FakeHooks(
        Feature.CLIPBOARD,
        FeatureAvailability.FEATURE_AVAILABILITY_DISABLED,
        takes = setOf(Envelope.PayloadCase.CLIPBOARD_PUSH, Envelope.PayloadCase.FEATURE_STATUS),
    )
    private val composite = CompositeSessionHooks(notifications, clipboard)

    private val desktop = PairedDevice(
        device = Device(DeviceId("desk"), "Desk", DeviceKind.DESKTOP),
        fingerprint = KeyFingerprint.of(ByteArray(32) { 1 }),
        pairedAtEpochMillis = 0,
        presenceKey = PresenceKey.of(ByteArray(32) { 9 }),
    )
    private val session = Session(
        object : Transport {
            override val input = ByteArrayInputStream(ByteArray(0))
            override val output = ByteArrayOutputStream()
            override fun close() = Unit
        },
    )

    private companion object {
        fun status(vararg states: Pair<Feature, FeatureAvailability>) = featureStatus {
            states.forEach { (feature, availability) ->
                features += featureState {
                    this.feature = feature.wireName
                    this.availability = availability
                }
            }
        }
    }

    @Test
    fun theFeaturesAndTheirStatesAreCombined() {
        assertEquals(setOf(Feature.NOTIFICATIONS, Feature.CLIPBOARD), composite.features())
        assertEquals(
            status(Feature.NOTIFICATIONS to FeatureAvailability.FEATURE_AVAILABILITY_READY, Feature.CLIPBOARD to FeatureAvailability.FEATURE_AVAILABILITY_DISABLED),
            composite.featureStatus(),
        )
    }

    @Test
    fun eachChangeGivesTheCompleteStatusAndNoneIsMerged() = runBlocking {
        val seen = Channel<FeatureStatus>(Channel.UNLIMITED)
        val collecting = launch(start = CoroutineStart.UNDISPATCHED) { composite.featureStatusChanges().collect { seen.send(it) } }

        // Away and back at once, as a restarted notification listener does: both go out.
        notifications.change(FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED)
        notifications.change(FeatureAvailability.FEATURE_AVAILABILITY_READY)
        clipboard.change(FeatureAvailability.FEATURE_AVAILABILITY_READY)

        withTimeout(5_000) {
            assertEquals(
                status(
                    Feature.NOTIFICATIONS to FeatureAvailability.FEATURE_AVAILABILITY_PERMISSION_REQUIRED,
                    Feature.CLIPBOARD to FeatureAvailability.FEATURE_AVAILABILITY_DISABLED,
                ),
                seen.receive(),
            )
            assertEquals(
                status(Feature.NOTIFICATIONS to FeatureAvailability.FEATURE_AVAILABILITY_READY, Feature.CLIPBOARD to FeatureAvailability.FEATURE_AVAILABILITY_DISABLED),
                seen.receive(),
            )
            assertEquals(
                status(Feature.NOTIFICATIONS to FeatureAvailability.FEATURE_AVAILABILITY_READY, Feature.CLIPBOARD to FeatureAvailability.FEATURE_AVAILABILITY_READY),
                seen.receive(),
            )
        }
        collecting.cancel()
    }

    @Test
    fun aChangeRightAfterCollectingStartsIsNotLost() = runBlocking {
        val seen = Channel<FeatureStatus>(Channel.UNLIMITED)
        val collecting = launch(start = CoroutineStart.UNDISPATCHED) { composite.featureStatusChanges().collect { seen.send(it) } }

        // Nothing else has run yet on this thread, so every hook must already be subscribed.
        clipboard.change(FeatureAvailability.FEATURE_AVAILABILITY_READY)

        val status = withTimeout(5_000) { seen.receive() }
        assertEquals(FeatureAvailability.FEATURE_AVAILABILITY_READY, status.featuresList.single { it.feature == "clipboard" }.availability)
        collecting.cancel()
    }

    @Test
    fun theDesktopsStatusReachesEveryHookAndOtherMessagesTheFirstThatTakesThem() = runBlocking {
        assertTrue(composite.onMessage(desktop, session, envelope { featureStatus = status() }))
        assertEquals(listOf(Envelope.PayloadCase.FEATURE_STATUS), notifications.messages)
        assertEquals(listOf(Envelope.PayloadCase.FEATURE_STATUS), clipboard.messages)

        assertTrue(composite.onMessage(desktop, session, envelope { notificationSyncRequest = notificationSyncRequest.defaultInstanceForType }))
        assertEquals(1, clipboard.messages.size)

        assertTrue(composite.onMessage(desktop, session, envelope { clipboardPush = clipboardPush.defaultInstanceForType }))
        assertEquals(Envelope.PayloadCase.CLIPBOARD_PUSH, clipboard.messages.last())

        // Nobody takes it, so the session answers a request with UNSUPPORTED.
        assertFalse(composite.onMessage(desktop, session, envelope { ping = ping { } }))
    }

    @Test
    fun everyHookHearsTheSessionEndEvenWhenOneFails() = runBlocking {
        notifications.failOnEnd = true

        val error = runCatching { composite.onEnded(desktop, session) }.exceptionOrNull()

        assertTrue("got $error", error is IllegalStateException)
        assertEquals(listOf(session), notifications.ended)
        assertEquals(listOf(session), clipboard.ended)
    }
}
