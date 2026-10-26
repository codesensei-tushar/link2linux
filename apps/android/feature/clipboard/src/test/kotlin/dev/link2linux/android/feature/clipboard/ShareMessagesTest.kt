package dev.link2linux.android.feature.clipboard

import dev.link2linux.android.core.clipboard.DesktopOutcome
import dev.link2linux.android.core.clipboard.PushOutcome
import dev.link2linux.android.core.clipboard.ShareResult
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

class ShareMessagesTest {

    private fun pushed(vararg outcomes: Pair<String, PushOutcome>) =
        ShareResult.Pushed(outcomes.map { (name, outcome) -> DesktopOutcome(name, outcome) })

    @Test
    fun aShareThatWentNowhereSaysWhy() {
        assertEquals(Message(R.string.clipboard_result_off_here), messageFor(ShareResult.Disabled))
        assertEquals(Message(R.string.clipboard_result_no_computer), messageFor(ShareResult.NoDesktop))
        assertEquals(Message(R.string.clipboard_result_nothing), messageFor(ShareResult.Empty))
        assertEquals(Message(R.string.clipboard_result_too_long), messageFor(ShareResult.TooLarge))
    }

    @Test
    fun theComputersThatGotItAreNamed() {
        assertEquals(
            Message(R.string.clipboard_result_sent, listOf("Desk, Laptop")),
            messageFor(pushed("Desk" to PushOutcome.SENT, "Laptop" to PushOutcome.SENT)),
        )
    }

    @Test
    fun aPartlySentShareNamesBothSides() {
        assertEquals(
            Message(R.string.clipboard_result_sent_partly, listOf("Desk", "Laptop, Tower")),
            messageFor(pushed("Desk" to PushOutcome.SENT, "Laptop" to PushOutcome.TURNED_OFF, "Tower" to PushOutcome.FAILED)),
        )
    }

    @Test
    fun aComputerWithTheFeatureOffIsToldApartFromAFailure() {
        assertEquals(
            Message(R.string.clipboard_result_off_there, listOf("Desk")),
            messageFor(pushed("Desk" to PushOutcome.TURNED_OFF)),
        )
        assertEquals(
            Message(R.string.clipboard_result_failed, listOf("Desk, Laptop")),
            messageFor(pushed("Desk" to PushOutcome.TURNED_OFF, "Laptop" to PushOutcome.FAILED)),
        )
    }

    /**
     * The toast is what shows that another app sent text through "Copy to
     * computer", so every share says something, success included. Only a
     * share a newer one replaced before it went out stays quiet: the newer
     * one has its own message.
     */
    @Test
    fun everyShareSaysSomethingUnlessItNeverWentOut() {
        for (result in listOf(ShareResult.Disabled, ShareResult.NoDesktop, ShareResult.Empty, ShareResult.TooLarge)) {
            assertNotNull("$result", messageFor(result))
        }
        val outcomes = PushOutcome.entries
        for (first in outcomes) for (second in outcomes) {
            for (desktops in listOf(listOf(first), listOf(first, second))) {
                val result = ShareResult.Pushed(desktops.mapIndexed { i, outcome -> DesktopOutcome("Desk $i", outcome) })
                val replacedOnly = desktops.all { it == PushOutcome.REPLACED }
                assertEquals("$desktops", replacedOnly, messageFor(result) == null)
            }
        }
    }

    @Test
    fun aShareANewerOneReplacedSaysNothing() {
        assertNull(messageFor(pushed("Desk" to PushOutcome.REPLACED)))
        // The replaced part does not count as a failure.
        assertEquals(
            Message(R.string.clipboard_result_sent, listOf("Laptop")),
            messageFor(pushed("Desk" to PushOutcome.REPLACED, "Laptop" to PushOutcome.SENT)),
        )
    }
}
