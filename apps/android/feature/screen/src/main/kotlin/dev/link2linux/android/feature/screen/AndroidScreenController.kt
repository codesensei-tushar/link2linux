package dev.link2linux.android.feature.screen

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.accessibility.AccessibilityNodeInfo
import dev.link2linux.android.core.screen.ScreenController
import dev.link2linux.android.core.screen.Stroke
import dev.link2linux.android.core.screen.TextField
import dev.link2linux.protocol.v1.ScreenKey
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/**
 * Carries out the desktop's input through the app's accessibility service
 * ([service], null while it does not run): gestures as strokes of one
 * finger, the navigation keys as the system's global actions, and typing
 * on the text field that has the input focus.
 */
internal class AndroidScreenController(private val service: () -> AccessibilityService?) : ScreenController {
    private val main = Handler(Looper.getMainLooper())

    /**
     * Each stroke continues the one before with the finger still down, and
     * goes out once the one before is over, so that a finger that rests
     * before it moves rests as long as the desktop's user held it.
     */
    override suspend fun gesture(strokes: List<Stroke>): Boolean {
        val service = service() ?: return false
        var previous: GestureDescription.StrokeDescription? = null
        for ((i, stroke) in strokes.withIndex()) {
            val path = Path().apply {
                moveTo(stroke.points[0].x, stroke.points[0].y)
                for (point in stroke.points.drop(1)) lineTo(point.x, point.y)
            }
            val more = i < strokes.lastIndex
            val description = try {
                previous?.continueStroke(path, 0, stroke.durationMillis, more)
                    ?: GestureDescription.StrokeDescription(path, 0, stroke.durationMillis, more)
            } catch (_: IllegalArgumentException) {
                return false
            }
            if (!dispatch(service, GestureDescription.Builder().addStroke(description).build())) return false
            previous = description
        }
        return true
    }

    override fun navigate(key: ScreenKey): Boolean {
        val action = when (key) {
            ScreenKey.SCREEN_KEY_BACK -> AccessibilityService.GLOBAL_ACTION_BACK
            ScreenKey.SCREEN_KEY_HOME -> AccessibilityService.GLOBAL_ACTION_HOME
            ScreenKey.SCREEN_KEY_RECENTS -> AccessibilityService.GLOBAL_ACTION_RECENTS
            ScreenKey.SCREEN_KEY_NOTIFICATIONS -> AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS
            else -> return false
        }
        return service()?.performGlobalAction(action) == true
    }

    override fun focusedField(): TextField? {
        val node = service()?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return null
        // Android may answer from a cache: the field must still be there, hold the focus and take text.
        if (!node.refresh() || !node.isFocused || !node.isEditable) return null
        return NodeField(node)
    }

    private suspend fun dispatch(service: AccessibilityService, gesture: GestureDescription): Boolean =
        suspendCancellableCoroutine { done ->
            val callback = object : AccessibilityService.GestureResultCallback() {
                override fun onCompleted(gestureDescription: GestureDescription?) {
                    if (done.isActive) done.resume(true)
                }

                override fun onCancelled(gestureDescription: GestureDescription?) {
                    if (done.isActive) done.resume(false)
                }
            }
            if (!service.dispatchGesture(gesture, callback, main) && done.isActive) done.resume(false)
        }

    /** The focused field as it was when looked up; writing replaces its text and places the cursor. */
    private class NodeField(private val node: AccessibilityNodeInfo) : TextField {
        // Nodes are equal when they stand for the same view in the same window.
        override val identity: Any = node

        // A field that shows its hint has no content.
        override val text: String = if (node.isShowingHintText) "" else node.text?.toString().orEmpty()
        override val selectionStart: Int = node.textSelectionStart
        override val selectionEnd: Int = node.textSelectionEnd
        override val isPassword: Boolean = node.isPassword
        override val isMultiLine: Boolean = node.isMultiLine
        override val canEnter: Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            node.actionList.any { it.id == AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id }

        override fun setText(text: String, cursor: Int): Boolean {
            val set = node.performAction(
                AccessibilityNodeInfo.ACTION_SET_TEXT,
                Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) },
            )
            if (set) {
                node.performAction(
                    AccessibilityNodeInfo.ACTION_SET_SELECTION,
                    Bundle().apply {
                        putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, cursor)
                        putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, cursor)
                    },
                )
            }
            return set
        }

        override fun enter(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
    }
}
