package dev.link2linux.android.feature.photos

import android.Manifest
import dev.link2linux.android.core.photos.PhotoAvailability
import dev.link2linux.protocol.v1.photo
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PhotoRowsTest {

    private fun row(
        taken: Long? = 1_700_000_000_123,
        orientation: Int = 0,
        name: String? = "IMG_1.jpg",
    ) = PhotoRow(
        id = 42,
        name = name,
        mimeType = "image/jpeg",
        size = 3_000_000,
        takenUnixMs = taken,
        addedUnixSeconds = 1_600_000_000,
        modifiedUnixSeconds = 1_650_000_000,
        width = 4000,
        height = 3000,
        orientation = orientation,
    )

    @Test
    fun aRowBecomesThePhotoTheDesktopSees() {
        val photo = row().toPhoto()

        assertEquals("42", photo.id)
        assertEquals(1_700_000_000_123, photo.takenUnixMs)
        assertEquals(1_650_000_000_000, photo.modifiedUnixMs)
        assertEquals("IMG_1.jpg", photo.name)
        assertEquals("image/jpeg", photo.mimeType)
        assertEquals(3_000_000, photo.size)
        assertEquals(4000 to 3000, photo.width to photo.height)
    }

    @Test
    fun aPhotoWithoutATakenTimeWasTakenWhenItWasAdded() {
        assertEquals(1_600_000_000_000, row(taken = null).toPhoto().takenUnixMs)
        assertEquals(1_600_000_000_000, row(taken = 0).toPhoto().takenUnixMs)
    }

    @Test
    fun aPhotoOnItsSideSwapsItsSize() {
        assertEquals(3000 to 4000, row(orientation = 90).toPhoto().let { it.width to it.height })
        assertEquals(3000 to 4000, row(orientation = 270).toPhoto().let { it.width to it.height })
        assertEquals(4000 to 3000, row(orientation = 180).toPhoto().let { it.width to it.height })
    }

    @Test
    fun aRowWithoutANameHasAnEmptyOneTheEngineReplaces() {
        assertEquals("", row(name = null).toPhoto().name)
    }

    @Test
    fun theSelectionFindsCameraPhotosAndScreenshotsThatAreDone() {
        assertEquals(
            "(relative_path LIKE ? OR relative_path LIKE ?) AND is_pending = 0",
            PhotoQuery.selection("relative_path", "is_pending"),
        )
        assertEquals(listOf("DCIM/%", "Pictures/Screenshots/%"), PhotoQuery.RELATIVE_PATHS)
    }

    @Test
    fun theLegacySelectionMatchesPathsUnderTheRoots() {
        val (selection, args) = PhotoQuery.legacySelection("_data", listOf("/sdcard/DCIM", "/sdcard/Pictures/Screenshots/"))

        assertEquals("_data LIKE ? OR _data LIKE ?", selection)
        assertArrayEquals(arrayOf("/sdcard/DCIM/%", "/sdcard/Pictures/Screenshots/%"), args)
    }

    @Test
    fun theSampleSizeKeepsAtLeastTheSizeAskedFor() {
        assertEquals(1, PhotoQuery.sampleSize(4000, 4000))
        assertEquals(1, PhotoQuery.sampleSize(300, 2048))
        assertEquals(1, PhotoQuery.sampleSize(4000, 2048))
        assertEquals(2, PhotoQuery.sampleSize(4096, 2048))
        assertEquals(8, PhotoQuery.sampleSize(4000, 400))
    }

    @Test
    fun fittingScalesDownAndNeverUp() {
        assertEquals(2048 to 1536, PhotoQuery.fit(4000, 3000, 2048))
        assertEquals(1536 to 2048, PhotoQuery.fit(3000, 4000, 2048))
        assertEquals(300 to 200, PhotoQuery.fit(300, 200, 2048))
        assertEquals(64 to 1, PhotoQuery.fit(10_000, 10, 64))
    }

    @Test
    fun onlyIdsThePhoneGivesOutNameAPhoto() {
        assertEquals(42L, PhotoQuery.mediaId(photo { id = "42" }))
        assertNull(PhotoQuery.mediaId(photo { id = "-1" }))
        assertNull(PhotoQuery.mediaId(photo { id = "042" }))
        assertNull(PhotoQuery.mediaId(photo { id = "42/../7" }))
    }

    @Test
    fun thePermissionsAskedForFollowTheAndroidVersion() {
        assertArrayEquals(
            arrayOf(Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED),
            PhotoPermissions.requested(34),
        )
        assertArrayEquals(arrayOf(Manifest.permission.READ_MEDIA_IMAGES), PhotoPermissions.requested(33))
        assertArrayEquals(arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE), PhotoPermissions.requested(32))
    }

    @Test
    fun accessFollowsWhatTheUserAllowed() {
        val images = Manifest.permission.READ_MEDIA_IMAGES
        val picked = Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED
        val storage = Manifest.permission.READ_EXTERNAL_STORAGE

        assertEquals(PhotoAccess.ALL, PhotoPermissions.accessOf(34) { it == images || it == picked })
        assertEquals(PhotoAccess.SELECTED, PhotoPermissions.accessOf(34) { it == picked })
        assertEquals(PhotoAccess.NONE, PhotoPermissions.accessOf(34) { false })
        assertEquals(PhotoAccess.ALL, PhotoPermissions.accessOf(33) { it == images })
        // Storage access no longer counts for images from Android 13 on.
        assertEquals(PhotoAccess.NONE, PhotoPermissions.accessOf(33) { it == storage })
        assertEquals(PhotoAccess.ALL, PhotoPermissions.accessOf(28) { it == storage })
        assertEquals(PhotoAccess.NONE, PhotoPermissions.accessOf(28) { it == images })
    }

    @Test
    fun theFeatureIsReadyOnceOnAndAllowed() {
        assertEquals(PhotoAvailability.DISABLED, PhotoPermissions.availabilityOf(false, PhotoAccess.ALL))
        assertEquals(PhotoAvailability.PERMISSION_REQUIRED, PhotoPermissions.availabilityOf(true, PhotoAccess.NONE))
        assertEquals(PhotoAvailability.READY, PhotoPermissions.availabilityOf(true, PhotoAccess.SELECTED))
        assertEquals(PhotoAvailability.READY, PhotoPermissions.availabilityOf(true, PhotoAccess.ALL))
    }
}
