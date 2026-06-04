package dev.zed.gpui

import android.content.Context
import android.graphics.Rect
import android.os.Build
import android.text.InputType
import android.util.AttributeSet
import android.util.Log
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.VelocityTracker
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * Custom SurfaceView for GPUI Android rendering.
 *
 * Owns the surface lifecycle, touch / fling / IME forwarding, and IME inset
 * reporting. Calls into the `gpui_android` Rust crate via JNI; downstream apps
 * just instantiate this class and place it in their layout.
 */
class GpuiSurfaceView
    @JvmOverloads
    constructor(
        context: Context,
        attrs: AttributeSet? = null,
        defStyleAttr: Int = 0,
    ) : SurfaceView(context, attrs, defStyleAttr), SurfaceHolder.Callback {
        companion object {
            private const val TAG = "GpuiSurfaceView"

            private const val ACTION_DOWN = 0
            private const val ACTION_UP = 1
            private const val ACTION_MOVE = 2
            private const val ACTION_CANCEL = 3
            private const val KEY_ACTION_DOWN = 0
            private const val FLING_VELOCITY_THRESHOLD = 150f
            private const val GESTURE_EXCLUSION_DP = 60f

            @JvmStatic private external fun nativeSurfaceCreated(surface: android.view.Surface)

            @JvmStatic private external fun nativeSurfaceChanged(
                format: Int,
                width: Int,
                height: Int,
            )

            @JvmStatic private external fun nativeSurfaceDestroyed()

            @JvmStatic private external fun nativeTouchEvent(
                action: Int,
                x: Float,
                y: Float,
                pointerId: Int,
            )

            @JvmStatic private external fun nativeKeyEvent(
                action: Int,
                keyCode: Int,
                unicode: Int,
            )

            @JvmStatic private external fun nativeImeInput(text: String)

            @JvmStatic private external fun nativeImeSetComposingText(
                text: String,
                newCursorPosition: Int,
            )

            @JvmStatic private external fun nativeImeFinishComposingText()

            @JvmStatic private external fun nativeFlingEvent(
                velocityX: Float,
                velocityY: Float,
            )

            @JvmStatic private external fun nativeLongPressEvent(x: Float, y: Float)

            @JvmStatic private external fun nativeKeyboardHeightChanged(height: Int)

            @JvmStatic private external fun nativeSystemInsetsChanged(
                top: Int,
                bottom: Int,
            )
        }

        private var velocityTracker: VelocityTracker? = null
        private var keyboardRequested = false
        private var imeHeight = 0
        private var keyboardAccessoryHeight = 0

        private val gestureDetector =
            GestureDetector(
                context,
                object : GestureDetector.SimpleOnGestureListener() {
                    override fun onLongPress(e: MotionEvent) {
                        performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                        nativeLongPressEvent(e.x, e.y)
                    }
                },
            )

        init {
            holder.addCallback(this)
            isFocusable = true
            isFocusableInTouchMode = true

            ViewCompat.setOnApplyWindowInsetsListener(this) { _, insets ->
                val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
                imeHeight = ime.bottom
                pushKeyboardHeight()

                val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
                nativeSystemInsetsChanged(systemBars.top, systemBars.bottom)
                insets
            }
        }

        // Reserve a left-edge strip from system gesture navigation so apps can
        // detect drawer swipes. Android caps the rect height to 200dp; we ask
        // for the full view height and let the system clamp it.
        override fun onLayout(
            changed: Boolean,
            left: Int,
            top: Int,
            right: Int,
            bottom: Int,
        ) {
            super.onLayout(changed, left, top, right, bottom)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val edgeWidth = (GESTURE_EXCLUSION_DP * resources.displayMetrics.density).toInt()
                systemGestureExclusionRects = listOf(Rect(0, 0, edgeWidth, bottom - top))
            }
        }

        // ----- Soft keyboard -----

        fun requestKeyboard() {
            Log.d(TAG, "[DEBUG-ANDROID-IME] requestKeyboard before requested=$keyboardRequested focus=${hasFocus()}")
            keyboardRequested = true
            requestFocus()
            val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
            // The surface view is normally already focused. Restart the input
            // connection after flipping text-editor state so IME show is honored.
            imm?.restartInput(this)
            imm?.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
            Log.d(TAG, "[DEBUG-ANDROID-IME] requestKeyboard after requested=$keyboardRequested focus=${hasFocus()} imm=${imm != null}")
        }

        fun dismissKeyboard() {
            Log.d(TAG, "[DEBUG-ANDROID-IME] dismissKeyboard before requested=$keyboardRequested focus=${hasFocus()}")
            keyboardRequested = false
            val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
            imm?.restartInput(this)
            imm?.hideSoftInputFromWindow(windowToken, 0)
            Log.d(TAG, "[DEBUG-ANDROID-IME] dismissKeyboard after requested=$keyboardRequested imm=${imm != null}")
        }

        fun setKeyboardAccessoryHeight(height: Int) {
            keyboardAccessoryHeight = height.coerceAtLeast(0)
            pushKeyboardHeight()
        }

        private fun pushKeyboardHeight() {
            // Host-provided keyboard accessory bars occupy app content above the IME,
            // so GPUI keyboard avoidance must reserve both regions together.
            val accessoryHeight = if (imeHeight > 0) keyboardAccessoryHeight else 0
            Log.d(
                TAG,
                "[DEBUG-ANDROID-IME] pushKeyboardHeight ime=$imeHeight accessory=$accessoryHeight total=${imeHeight + accessoryHeight}",
            )
            nativeKeyboardHeightChanged(imeHeight + accessoryHeight)
        }

        override fun onCheckIsTextEditor(): Boolean {
            Log.d(TAG, "[DEBUG-ANDROID-IME] onCheckIsTextEditor requested=$keyboardRequested focus=${hasFocus()}")
            return keyboardRequested
        }

        override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
            Log.d(TAG, "[DEBUG-ANDROID-IME] onCreateInputConnection requested=$keyboardRequested focus=${hasFocus()}")
            outAttrs.inputType = InputType.TYPE_CLASS_TEXT
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE

            return object : BaseInputConnection(this, false) {
                override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
                    Log.d(
                        TAG,
                        "[DEBUG-ANDROID-IME] setComposingText text='${text?.toString().orEmpty()}' cursor=$newCursorPosition",
                    )
                    nativeImeSetComposingText(text?.toString().orEmpty(), newCursorPosition)
                    return true
                }

                override fun finishComposingText(): Boolean {
                    Log.d(TAG, "[DEBUG-ANDROID-IME] finishComposingText")
                    nativeImeFinishComposingText()
                    return true
                }

                override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                    val s = text?.toString().orEmpty()
                    Log.d(TAG, "[DEBUG-ANDROID-IME] commitText text='$s' cursor=$newCursorPosition")
                    if (s.isNotEmpty()) {
                        nativeImeInput(s)
                    }
                    return true
                }

                override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                    Log.d(
                        TAG,
                        "[DEBUG-ANDROID-IME] deleteSurroundingText before=$beforeLength after=$afterLength",
                    )
                    repeat(beforeLength.coerceAtLeast(0)) {
                        nativeKeyEvent(KEY_ACTION_DOWN, KeyEvent.KEYCODE_DEL, 0)
                    }
                    return true
                }

                override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
                    deleteSurroundingText(beforeLength, afterLength)

                override fun sendKeyEvent(event: KeyEvent): Boolean {
                    Log.d(
                        TAG,
                        "[DEBUG-ANDROID-IME] sendKeyEvent action=${event.action} keyCode=${event.keyCode} unicode=${event.unicodeChar}",
                    )
                    if (event.action == KeyEvent.ACTION_DOWN) {
                        nativeKeyEvent(KEY_ACTION_DOWN, event.keyCode, event.unicodeChar)
                    }
                    return true
                }
            }
        }

        // ----- Surface lifecycle -----

        override fun surfaceCreated(holder: SurfaceHolder) {
            Log.d(TAG, "surfaceCreated")
            nativeSurfaceCreated(holder.surface)
        }

        override fun surfaceChanged(
            holder: SurfaceHolder,
            format: Int,
            width: Int,
            height: Int,
        ) {
            Log.d(TAG, "surfaceChanged ${width}x$height format=$format")
            nativeSurfaceChanged(format, width, height)
        }

        override fun surfaceDestroyed(holder: SurfaceHolder) {
            Log.d(TAG, "surfaceDestroyed")
            nativeSurfaceDestroyed()
        }

        // ----- Input -----

        override fun onTouchEvent(event: MotionEvent): Boolean {
            gestureDetector.onTouchEvent(event)
            val action =
                when (event.actionMasked) {
                    MotionEvent.ACTION_DOWN -> {
                        velocityTracker?.recycle()
                        velocityTracker = VelocityTracker.obtain().also { it.addMovement(event) }
                        ACTION_DOWN
                    }
                    MotionEvent.ACTION_UP -> {
                        velocityTracker?.let { tracker ->
                            tracker.addMovement(event)
                            tracker.computeCurrentVelocity(1000)
                            val vx = tracker.xVelocity
                            val vy = tracker.yVelocity
                            if (kotlin.math.abs(vx) > FLING_VELOCITY_THRESHOLD || kotlin.math.abs(vy) > FLING_VELOCITY_THRESHOLD) {
                                nativeFlingEvent(vx, vy)
                            }
                            tracker.recycle()
                        }
                        velocityTracker = null
                        ACTION_UP
                    }
                    MotionEvent.ACTION_MOVE -> {
                        velocityTracker?.addMovement(event)
                        ACTION_MOVE
                    }
                    MotionEvent.ACTION_CANCEL -> {
                        velocityTracker?.recycle()
                        velocityTracker = null
                        ACTION_CANCEL
                    }
                    else -> return super.onTouchEvent(event)
                }

            val pointerIndex = event.actionIndex
            val pointerId = event.getPointerId(pointerIndex)

            // Android batches multiple touchscreen samples (typically 120-240 Hz)
            // into one MotionEvent (delivered at ~vsync). Native scrollers emit
            // an event per historical sample so the scroll position tracks the
            // finger at sub-frame resolution. Without this, scrolling jumps in
            // coarse steps and feels less smooth than RecyclerView / WebView.
            if (action == ACTION_MOVE) {
                val historySize = event.historySize
                for (h in 0 until historySize) {
                    val hx = event.getHistoricalX(pointerIndex, h)
                    val hy = event.getHistoricalY(pointerIndex, h)
                    nativeTouchEvent(action, hx, hy, pointerId)
                }
            }
            val x = event.getX(pointerIndex)
            val y = event.getY(pointerIndex)
            nativeTouchEvent(action, x, y, pointerId)
            return true
        }

        override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
            nativeKeyEvent(KEY_ACTION_DOWN, keyCode, event.unicodeChar)
            return true
        }
    }
