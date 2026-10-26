package dev.link2linux.android.feature.notifications

import android.graphics.drawable.Icon
import android.util.TypedValue
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ImageRenderingTest {

    @Test
    fun `a huge image is decoded at a fraction of its size, never below what is sent`() {
        // 20000 x 20000 pixels for a 256-pixel large icon: 64 x smaller, 312 pixels on a side.
        assertEquals(64, BoundedImages.sampleSize(20_000, 20_000, 256))
        assertEquals(1, BoundedImages.sampleSize(300, 200, 256))
        assertEquals(2, BoundedImages.sampleSize(512, 100, 256))
        assertEquals(1, BoundedImages.sampleSize(1, 1, 1024))
    }

    @Test
    fun `only a bitmap the notification holds is left to Android as it is`() {
        assertEquals(IconRoute.URI, IconRoute.of(Icon.TYPE_URI))
        assertEquals(IconRoute.URI, IconRoute.of(6)) // TYPE_URI_ADAPTIVE_BITMAP
        assertEquals(IconRoute.BITMAP, IconRoute.of(Icon.TYPE_BITMAP))
        assertEquals(IconRoute.BITMAP, IconRoute.of(Icon.TYPE_ADAPTIVE_BITMAP))
        // Android decodes these at full size, so the app checks their size first.
        assertEquals(IconRoute.DATA, IconRoute.of(Icon.TYPE_DATA))
        assertEquals(IconRoute.RESOURCE, IconRoute.of(Icon.TYPE_RESOURCE))
        // Before Android 9 the type comes from a method that is not public; if it cannot be read, no icon.
        assertEquals(IconRoute.SKIP, IconRoute.of(null))
        assertEquals(IconRoute.SKIP, IconRoute.of(42))
    }

    @Test
    fun `a resource bitmap counts at the size Android scales it to for the screen`() {
        // 5000 x 5000 in drawable-mdpi on a 640 dpi screen: Android decodes 20000 x 20000.
        assertEquals(400_000_000L, ResourceImages.scaledPixels(5000, 5000, 160, 640))
        assertEquals(400_000_000L, ResourceImages.scaledPixels(5000, 5000, TypedValue.DENSITY_DEFAULT, 640))
        assertEquals(25_000_000L, ResourceImages.scaledPixels(5000, 5000, 640, 640))
        assertEquals(25_000_000L, ResourceImages.scaledPixels(5000, 5000, TypedValue.DENSITY_NONE, 640))
        // From a denser folder it shrinks, rounding up.
        assertTrue(ResourceImages.scaledPixels(431, 431, 640, 160) == 108L * 108L)
    }
}
