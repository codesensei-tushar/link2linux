package dev.link2linux.android.ui

import dev.link2linux.android.core.model.Feature
import org.junit.Assert.assertEquals
import org.junit.Test

class FeatureRoutesTest {

    @Test
    fun everyOptInFeatureHasItsOwnRoute() {
        assertEquals(Feature.optIn, featureRoutes.keys.toList())
        assertEquals(featureRoutes.size, featureRoutes.values.toSet().size)
    }
}
