package dev.link2linux.android.core.screen

import dev.link2linux.protocol.v1.ErrorCode
import dev.link2linux.protocol.v1.ScreenGesture
import dev.link2linux.protocol.v1.ScreenInput
import dev.link2linux.protocol.v1.ScreenKey
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Carries out one `ScreenInput` through [controller] and gives the answer
 * (docs/protocol.md, "Control"). Whether the desktop may send input at all
 * is the stream's to check first.
 */
internal class InputCarrier(private val controller: ScreenController) {

    /** Carries [input] out on the screen whose video is [video], the current one. */
    suspend fun carryOut(input: ScreenInput, video: VideoFormat?): ScreenReply = when (input.inputCase) {
        ScreenInput.InputCase.GESTURE -> gesture(input.gesture, video)
        ScreenInput.InputCase.KEY -> key(input.key)
        ScreenInput.InputCase.TEXT -> text(input.text)
        else -> invalid("An input without a gesture, a key or text")
    }

    private suspend fun gesture(gesture: ScreenGesture, video: VideoFormat?): ScreenReply {
        val points = gesture.pointsList
        Gestures.shapeError(points)?.let { return invalid(it) }
        if (video == null || gesture.formatId != video.id) {
            return ScreenReply.Refused(ErrorCode.ERROR_CODE_EXPIRED, "A gesture on frames that are no longer sent")
        }
        if (!Gestures.inFrame(points, video)) return invalid("A gesture with a point outside the frame")
        val strokes = Gestures.strokes(points, video)
        val took = strokes.sumOf { it.durationMillis }
        // Android tells when the touch is over; one it never tells about must not hold up the inputs after it.
        val done = withTimeoutOrNull(took + GESTURE_GRACE_MILLIS) { controller.gesture(strokes) } ?: false
        return if (done) ScreenReply.Done else unavailable("Android did not carry the gesture out")
    }

    private fun key(key: ScreenKey): ScreenReply = when (key) {
        ScreenKey.SCREEN_KEY_BACK,
        ScreenKey.SCREEN_KEY_HOME,
        ScreenKey.SCREEN_KEY_RECENTS,
        ScreenKey.SCREEN_KEY_NOTIFICATIONS,
        -> if (controller.navigate(key)) ScreenReply.Done else unavailable("Android did not carry the key out")
        ScreenKey.SCREEN_KEY_BACKSPACE -> withField { field ->
            val edit = TextEdits.backspace(field.text, field.selectionStart, field.selectionEnd)
            if (edit == null) ScreenReply.Done else write(field, edit)
        }
        ScreenKey.SCREEN_KEY_ENTER -> withField { field ->
            when {
                field.canEnter -> if (field.enter()) ScreenReply.Done else unavailable("The text field did not take Enter")
                field.isMultiLine -> write(field, TextEdits.insert(field.text, field.selectionStart, field.selectionEnd, "\n"))
                // Not UNSUPPORTED, which would say the feature is not active in this session.
                else -> ScreenReply.Refused(ErrorCode.ERROR_CODE_NOT_ALLOWED, "The text field has no Enter")
            }
        }
        else -> invalid("An unknown key")
    }

    private fun text(text: String): ScreenReply {
        if (text.isEmpty() || text.toByteArray(Charsets.UTF_8).size > MAX_TEXT_BYTES) {
            return invalid("Text of ${text.length} chars, outside 1 to $MAX_TEXT_BYTES bytes")
        }
        return withField { field -> write(field, TextEdits.insert(field.text, field.selectionStart, field.selectionEnd, text)) }
    }

    /**
     * Runs [edit] on the focused text field. Android shows a password
     * field's content as dots, so typing can only replace all of it: into
     * one that is empty or wholly selected.
     */
    private fun withField(edit: (TextField) -> ScreenReply): ScreenReply {
        val field = controller.focusedField()
            ?: return ScreenReply.Refused(ErrorCode.ERROR_CODE_NOT_FOUND, "No text field has the focus")
        if (field.isPassword && field.text.isNotEmpty() && !TextEdits.selectsAll(field.text, field.selectionStart, field.selectionEnd)) {
            return ScreenReply.Refused(ErrorCode.ERROR_CODE_NOT_ALLOWED, "Android hides what a password field holds")
        }
        return edit(field)
    }

    private fun write(field: TextField, edit: TextEdits.Edit): ScreenReply =
        if (field.setText(edit.text, edit.cursor)) ScreenReply.Done else unavailable("The text field did not take the text")

    private fun invalid(detail: String) = ScreenReply.Refused(ErrorCode.ERROR_CODE_INVALID_ARGUMENT, detail)

    private fun unavailable(detail: String) = ScreenReply.Refused(ErrorCode.ERROR_CODE_UNAVAILABLE, detail)

    companion object {
        /** docs/protocol.md, "Control". */
        const val MAX_TEXT_BYTES = 4096

        /** How long past its own length a gesture may take before it counts as not carried out. */
        const val GESTURE_GRACE_MILLIS = 5_000L
    }
}
