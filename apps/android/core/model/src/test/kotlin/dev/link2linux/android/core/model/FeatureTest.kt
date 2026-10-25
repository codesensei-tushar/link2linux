package dev.link2linux.android.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class FeatureTest {

    @Test
    fun wireNamesMatchTheDesktopFeatureNames() {
        assertEquals(
            listOf("pairing", "notifications", "messages", "calls", "photos", "files", "clipboard", "screen"),
            Feature.entries.map(Feature::wireName),
        )
    }

    @Test
    fun fromWireNameRoundTrips() {
        Feature.entries.forEach { feature ->
            assertEquals(feature, Feature.fromWireName(feature.wireName))
        }
    }

    @Test
    fun unknownWireNameIsIgnored() {
        assertNull(Feature.fromWireName("teleport"))
        assertNull(Feature.fromWireName("Notifications"))
    }

    @Test
    fun pairingIsNotOptIn() {
        assertFalse(Feature.PAIRING.isOptIn)
        assertEquals(Feature.entries - Feature.PAIRING, Feature.optIn)
    }
}
