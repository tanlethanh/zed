package dev.zed.gpui

import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection

/**
 * Bridges Android input-method callbacks to the active editable GPUI input handler.
 *
 * Native touch selection is owned separately by [SelectionController].
 */
internal class InputMethodAdapter(
    targetView: View,
    private val commitTextToGpui: (String) -> Unit,
    private val setComposingTextInGpui: (String, Int) -> Unit,
    private val finishComposingTextInGpui: () -> Unit,
    private val deleteBackwardInGpui: () -> Unit,
    private val sendKeyEventToGpui: (KeyEvent) -> Unit,
) : BaseInputConnection(targetView, false) {
    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        setComposingTextInGpui(text?.toString().orEmpty(), newCursorPosition)
        return true
    }

    override fun finishComposingText(): Boolean {
        finishComposingTextInGpui()
        return true
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        text?.toString()?.takeIf(String::isNotEmpty)?.let(commitTextToGpui)
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        repeat(beforeLength.coerceAtLeast(0)) {
            deleteBackwardInGpui()
        }
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
        deleteSurroundingText(beforeLength, afterLength)

    override fun sendKeyEvent(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_DOWN) {
            sendKeyEventToGpui(event)
        }
        return true
    }
}
