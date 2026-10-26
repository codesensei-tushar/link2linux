package dev.link2linux.android.feature.notifications

import android.graphics.drawable.Icon
import org.junit.Assert.assertEquals
import org.junit.Test

class UriImagesTest {

    @Test
    fun `a huge image is decoded at a fraction of its size, never below what is sent`() {
        // 20000 x 20000 pixels for a 256-pixel large icon: 64 x smaller, 312 pixels on a side.
        assertEquals(64, UriImages.sampleSize(20_000, 20_000, 256))
        assertEquals(1, UriImages.sampleSize(300, 200, 256))
        assertEquals(2, UriImages.sampleSize(512, 100, 256))
        assertEquals(1, UriImages.sampleSize(1, 1, 1024))
    }

    @Test
    fun `only icons of a known type other than a URI are left to Android`() {
        assertEquals(IconRoute.URI, IconRoute.of(Icon.TYPE_URI))
        assertEquals(IconRoute.URI, IconRoute.of(6)) // TYPE_URI_ADAPTIVE_BITMAP
        assertEquals(IconRoute.LOAD, IconRoute.of(Icon.TYPE_BITMAP))
        assertEquals(IconRoute.LOAD, IconRoute.of(Icon.TYPE_RESOURCE))
        // Before Android 9 the type comes from a method that is not public; if it cannot be read, no icon.
        assertEquals(IconRoute.SKIP, IconRoute.of(null))
    }
}
