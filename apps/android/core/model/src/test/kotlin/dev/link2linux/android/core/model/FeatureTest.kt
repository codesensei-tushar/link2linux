package dev.link2linux.android.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class FeatureTest {

    @Test
    fun wireNamesMatchTheDesktopFeatureNames() {
        assertEquals(
            listOf("pairing", "notifications", "calls", "photos", "files", "clipboard", "screen", "status", "ring"),
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
    fun everyFeatureButPairingCanBeTurnedOff() {
        assertFalse(Feature.PAIRING.isSwitchable)
        assertEquals(Feature.entries - Feature.PAIRING, Feature.switchable)
    }

    @Test
    fun onlyFeaturesThatShareNothingStartOn() {
        assertEquals(listOf(Feature.CALLS, Feature.STATUS, Feature.RING), Feature.entries.filter(Feature::isOnByDefault))
    }
}
