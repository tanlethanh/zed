package dev.zed.gpui

import android.app.Activity
import android.os.Build
import android.util.Log
import android.view.Choreographer
import android.view.View
import android.view.Window
import android.widget.FrameLayout
import java.lang.ref.WeakReference

/**
 * Bridges an [Activity] lifecycle to the GPUI Android framework.
 *
 * Mirrors the role of `GPUIRuntimeController` in `gpui_ios/ios/Zedra/`. The
 * downstream app constructs one of these in `onCreate`, calls [launch] after
 * the Rust-side `Application::run` has been triggered, and forwards the four
 * lifecycle methods. The framework owns everything else.
 *
 * Typical usage (downstream `MainActivity.onCreate`):
 *
 *   val runtime = GpuiRuntimeController(this)
 *   runtime.attach(rootView)
 *   runtime.launchAfter { zedraLaunchGpui() }       // your Rust entry point
 *   runtime.didFinishLaunching()
 *
 * Then forward `onResume`, `onPause`, `onStop`, `onDestroy`.
 */
class GpuiRuntimeController(private val activity: Activity) {
    private val choreographer: Choreographer = Choreographer.getInstance()
    private var surfaceView: GpuiSurfaceView? = null
    private var isRunning = false

    private val frameCallback =
        object : Choreographer.FrameCallback {
            override fun doFrame(frameTimeNanos: Long) {
                if (!isRunning) return
                gpuiRequestFrame()
                choreographer.postFrameCallback(this)
            }
        }

    /**
     * Initialize the framework with the host [Activity] and store the JVM /
     * activity references the platform needs for its JNI callbacks.
     */
    fun initialize() {
        configureEdgeToEdge()
        gpuiInit(activity)
        setDisplayScale(activity.resources.displayMetrics.density)
    }

    /**
     * Insert a framework [GpuiSurfaceView] into the given root layout. Returns
     * the created view so the host can register it with auxiliary subsystems
     * (keyboard helpers, native presentations, etc.).
     */
    fun attach(rootView: FrameLayout): GpuiSurfaceView {
        val view = GpuiSurfaceView(activity)
        rootView.addView(
            view,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT,
            ),
        )
        view.requestFocus()
        surfaceView = view
        activeSurfaceView = WeakReference(view)
        return view
    }

    /** Returns the framework surface view, if attached. */
    fun surfaceView(): GpuiSurfaceView? = surfaceView

    /** Notify GPUI that the application has finished launching. */
    fun didFinishLaunching() {
        gpuiDidFinishLaunching()
    }

    fun onResume() {
        gpuiResume()
        isRunning = true
        choreographer.postFrameCallback(frameCallback)
    }

    fun onPause() {
        isRunning = false
        gpuiPause()
    }

    fun onStop() {
        isRunning = false
    }

    fun onDestroy() {
        isRunning = false
        if (activeSurfaceView?.get() === surfaceView) {
            activeSurfaceView = null
        }
        gpuiDestroy()
    }

    private fun configureEdgeToEdge() {
        val window: Window = activity.window
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = (
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                    View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                    View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
            )
        }
    }

    companion object {
        private const val TAG = "GpuiRuntimeController"
        private var activeSurfaceView: WeakReference<GpuiSurfaceView>? = null

        @JvmStatic
        fun requestSoftKeyboard() {
            val view = activeSurfaceView?.get() ?: return
            view.post { activeSurfaceView?.get()?.requestKeyboard() }
        }

        @JvmStatic
        fun hideSoftKeyboard() {
            val view = activeSurfaceView?.get() ?: return
            view.post { activeSurfaceView?.get()?.dismissKeyboard() }
        }

        @JvmStatic external fun gpuiInit(activity: Activity)

        @JvmStatic external fun gpuiDidFinishLaunching()

        @JvmStatic external fun gpuiResume()

        @JvmStatic external fun gpuiPause()

        @JvmStatic external fun gpuiDestroy()

        @JvmStatic external fun gpuiRequestFrame()

        @JvmStatic external fun gpuiRequestFrameForced()

        @JvmStatic external fun setDisplayScale(scale: Float)
    }
}
