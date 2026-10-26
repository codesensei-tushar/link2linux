package dev.link2linux.android.feature.clipboard

import org.junit.Assert.assertEquals
import org.junit.Test

class EntryPointsTest {

    @Test
    fun theTileNamesTheComputersOnlyWhileUnlocked() {
        assertEquals(Message(R.string.clipboard_send_to, listOf("Desk, Laptop")), tileSubtitle(true, listOf("Desk", "Laptop"), locked = false))
        assertEquals(Message(R.string.clipboard_tile_connected), tileSubtitle(true, listOf("Desk"), locked = true))
        assertEquals(Message(R.string.clipboard_tile_not_connected), tileSubtitle(true, emptyList(), locked = false))
        assertEquals(Message(R.string.clipboard_tile_off), tileSubtitle(false, listOf("Desk"), locked = false))
    }

    @Test
    fun handedOverTextIsSentAndAFileIsLeftToTheFileFeature() {
        assertEquals(Handed.Text("hi"), handedOver(hasFile = { false }, text = { "hi" }))
        assertEquals(Handed.Text(null), handedOver(hasFile = { false }, text = { null }))
        assertEquals(Handed.File, handedOver(hasFile = { true }, text = { "caption" }))
    }

    @Test
    fun extrasThatCannotBeReadCountAsNothingToShare() {
        // What an app's Bundle with a class this app cannot load throws when read.
        val unreadable = { throw RuntimeException("Parcelable encountered ClassNotFoundException") }

        assertEquals(Handed.Text(null), handedOver(hasFile = unreadable, text = { "hi" }))
        assertEquals(Handed.Text(null), handedOver(hasFile = { false }, text = unreadable))
    }
}
