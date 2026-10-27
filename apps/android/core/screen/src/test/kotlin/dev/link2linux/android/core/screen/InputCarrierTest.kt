package dev.link2linux.android.core.screen

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ScreenInput
import dev.link2linux.protocol.v1.ScreenKey
import dev.link2linux.protocol.v1.screenGesture
import dev.link2linux.protocol.v1.screenInput
import dev.link2linux.protocol.v1.screenPoint
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The fakes of the platform's controller, shared with the engine's tests. */
internal class FakeField(
    override var text: String,
    override var selectionStart: Int = text.length,
    override var selectionEnd: Int = selectionStart,
    override val isPassword: Boolean = false,
    override val isMultiLine: Boolean = false,
    override val canEnter: Boolean = false,
) : TextField {
    var accepts = true
    var entered = 0

    override fun setText(text: String, cursor: Int): Boolean {
        if (!accepts) return false
        this.text = text
        selectionStart = cursor
        selectionEnd = cursor
        return true
    }

    override fun enter(): Boolean {
        entered++
        return accepts
    }
}

internal class FakeController : ScreenController {
    /** What was carried out, in order. */
    val done = mutableListOf<Any>()
    var field: FakeField? = null
    var gestureWorks = true

    /** While set, a gesture lasts until it completes. */
    var gestureEnd: CompletableDeferred<Boolean>? = null

    override suspend fun gesture(strokes: List<Stroke>): Boolean {
        done.add(strokes)
        return gestureEnd?.await() ?: gestureWorks
    }

    override fun navigate(key: ScreenKey): Boolean {
        done += key
        return true
    }

    override fun focusedField(): TextField? = field
}

class InputCarrierTest {

    private val controller = FakeController()
    private val carrier = InputCarrier(controller)
    private val video = VideoFormat(id = 7, width = 540, height = 1200, screenWidth = 1080, screenHeight = 2400)

    private fun gesture(vararg points: Triple<Int, Int, Int>, formatId: Int = 7): ScreenInput = screenInput {
        gesture = screenGesture {
            this.formatId = formatId
            for ((x, y, t) in points) {
                this.points += screenPoint {
                    this.x = x
                    this.y = y
                    timeMs = t
                }
            }
        }
    }

    private fun key(key: ScreenKey) = screenInput { this.key = key }

    private fun text(text: String) = screenInput { this.text = text }

    private fun ScreenReply.code(): ErrorCode? = (this as? ScreenReply.Refused)?.code

    @Test
    fun aTapIsCarriedOutOnTheScreen() = runTest {
        val reply = carrier.carryOut(gesture(Triple(10, 20, 0)), video)

        assertEquals(ScreenReply.Done, reply)
        assertEquals(listOf(listOf(Stroke(listOf(ScreenPointF(21f, 41f)), Gestures.TAP_MILLIS))), controller.done)
    }

    @Test
    fun aGestureOnOldFramesHasExpired() = runTest {
        assertEquals(ErrorCode.ERROR_CODE_EXPIRED, carrier.carryOut(gesture(Triple(10, 20, 0), formatId = 6), video).code())
        assertEquals(ErrorCode.ERROR_CODE_EXPIRED, carrier.carryOut(gesture(Triple(10, 20, 0)), null).code())
        assertTrue(controller.done.isEmpty())
    }

    @Test
    fun aGestureOutOfRangeIsInvalid() = runTest {
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(gesture(Triple(540, 20, 0)), video).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(gesture(), video).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(screenInput { }, video).code())
        assertTrue(controller.done.isEmpty())
    }

    @Test
    fun aGestureAndroidDidNotCarryOutIsUnavailable() = runTest {
        controller.gestureWorks = false

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, carrier.carryOut(gesture(Triple(1, 1, 0)), video).code())
    }

    @Test
    fun aGestureAndroidNeverReportsOnGivesUpAfterItsLengthAndAGrace() = runTest {
        controller.gestureEnd = CompletableDeferred()
        val start = testScheduler.currentTime

        val reply = carrier.carryOut(gesture(Triple(1, 1, 0), Triple(1, 1, 2_000)), video)

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, reply.code())
        assertEquals(2_000 + InputCarrier.GESTURE_GRACE_MILLIS, testScheduler.currentTime - start)
    }

    @Test
    fun navigationKeysAreTheSystemsActions() = runTest {
        for (key in listOf(ScreenKey.SCREEN_KEY_BACK, ScreenKey.SCREEN_KEY_HOME, ScreenKey.SCREEN_KEY_RECENTS, ScreenKey.SCREEN_KEY_NOTIFICATIONS)) {
            assertEquals(ScreenReply.Done, carrier.carryOut(key(key), video))
        }
        assertEquals(
            listOf(ScreenKey.SCREEN_KEY_BACK, ScreenKey.SCREEN_KEY_HOME, ScreenKey.SCREEN_KEY_RECENTS, ScreenKey.SCREEN_KEY_NOTIFICATIONS),
            controller.done,
        )
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(key(ScreenKey.SCREEN_KEY_UNSPECIFIED), video).code())
    }

    @Test
    fun textIsTypedAtTheCursorInPlaceOfTheSelection() = runTest {
        val field = FakeField("hello world", selectionStart = 6, selectionEnd = 11).also { controller.field = it }

        assertEquals(ScreenReply.Done, carrier.carryOut(text("there"), video))

        assertEquals("hello there", field.text)
        assertEquals(11, field.selectionStart)
    }

    @Test
    fun textOutOfRangeIsInvalid() = runTest {
        controller.field = FakeField("")

        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(text(""), video).code())
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(text("x".repeat(4097)), video).code())
        // 4096 bytes in 2048 two-byte characters is fine, 4098 is not.
        assertEquals(ScreenReply.Done, carrier.carryOut(text("\u00e9".repeat(2048)), video))
        assertEquals(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, carrier.carryOut(text("\u00e9".repeat(2049)), video).code())
    }

    @Test
    fun withoutAFocusedFieldTextAndTextKeysAreNotFound() = runTest {
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, carrier.carryOut(text("a"), video).code())
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, carrier.carryOut(key(ScreenKey.SCREEN_KEY_BACKSPACE), video).code())
        assertEquals(ErrorCode.ERROR_CODE_NOT_FOUND, carrier.carryOut(key(ScreenKey.SCREEN_KEY_ENTER), video).code())
    }

    @Test
    fun backspaceDeletesBeforeTheCursorAndIsFineInAnEmptyField() = runTest {
        val field = FakeField("abc").also { controller.field = it }

        assertEquals(ScreenReply.Done, carrier.carryOut(key(ScreenKey.SCREEN_KEY_BACKSPACE), video))
        assertEquals("ab", field.text)

        field.text = ""
        assertEquals(ScreenReply.Done, carrier.carryOut(key(ScreenKey.SCREEN_KEY_BACKSPACE), video))
    }

    @Test
    fun enterUsesTheFieldsActionElseANewLineInAMultiLineField() = runTest {
        val action = FakeField("a", canEnter = true).also { controller.field = it }
        assertEquals(ScreenReply.Done, carrier.carryOut(key(ScreenKey.SCREEN_KEY_ENTER), video))
        assertEquals(1, action.entered)

        val lines = FakeField("a", isMultiLine = true).also { controller.field = it }
        assertEquals(ScreenReply.Done, carrier.carryOut(key(ScreenKey.SCREEN_KEY_ENTER), video))
        assertEquals("a\n", lines.text)

        controller.field = FakeField("a")
        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, carrier.carryOut(key(ScreenKey.SCREEN_KEY_ENTER), video).code())
    }

    @Test
    fun aPasswordFieldIsTypedIntoOnlyWhenItsWholeContentGoes() = runTest {
        // Android shows the password as dots: inserting among them would replace the password with dots.
        val password = FakeField("\u2022\u2022\u2022", isPassword = true).also { controller.field = it }
        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, carrier.carryOut(text("x"), video).code())
        assertEquals(ErrorCode.ERROR_CODE_NOT_ALLOWED, carrier.carryOut(key(ScreenKey.SCREEN_KEY_BACKSPACE), video).code())

        password.selectionStart = 0
        assertEquals(ScreenReply.Done, carrier.carryOut(text("secret"), video))
        assertEquals("secret", password.text)

        val empty = FakeField("", isPassword = true).also { controller.field = it }
        assertEquals(ScreenReply.Done, carrier.carryOut(text("pw"), video))
        assertEquals("pw", empty.text)
    }

    @Test
    fun aFieldThatRefusesTheTextIsUnavailable() = runTest {
        controller.field = FakeField("a").apply { accepts = false }

        assertEquals(ErrorCode.ERROR_CODE_UNAVAILABLE, carrier.carryOut(text("b"), video).code())
    }
}
