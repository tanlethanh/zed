package dev.zed.gpui

import android.app.SearchManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.PointF
import android.graphics.Rect
import android.graphics.RectF
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.util.TypedValue
import android.view.ActionMode
import android.view.Menu
import android.view.MenuItem
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.widget.FrameLayout
import android.widget.Magnifier
import java.text.BreakIterator
import java.lang.ref.WeakReference
import kotlin.math.hypot

/** Presents Android-native text selection for GPUI selection handlers. */
internal class SelectionController(
    private val rootView: FrameLayout,
    private val surfaceView: GpuiSurfaceView,
) {
    private data class NativeSelectionAction(
        val index: Int,
        val title: String,
    )

    private val handler = Handler(Looper.getMainLooper())
    private val touchSlop = ViewConfiguration.get(surfaceView.context).scaledTouchSlop.toFloat()
    private val longPressTimeout = ViewConfiguration.getLongPressTimeout().toLong()
    private val density = surfaceView.resources.displayMetrics.density
    private val handleRadius = 10f * density
    private val handleTouchRadius = maxOf(handleRadius * 1.8f, 24f * density)
    private val overlay = SelectionOverlayView(surfaceView, this)
    private var pendingLongPress: Runnable? = null
    private var downX = 0f
    private var downY = 0f
    private var longPressActive = false
    private var snapshot: SelectionSnapshot? = null
    private var actionMode: ActionMode? = null
    private var menuActions: List<NativeSelectionAction> = emptyList()
    private var lastLoggedSnapshot: SelectionSnapshot? = null
    private var dragOffsetX = 0f
    private var dragOffsetY = 0f
    // The word selected by the long press; the same-gesture drag grows from it.
    private var longPressAnchorStart = -1
    private var longPressAnchorEnd = -1
    // System loupe shown over the handle while dragging (API 28+). Magnifies the
    // SurfaceView's GPUI content at the handle's text line. Temporarily disabled:
    // anchored to the SurfaceView it can only capture GPUI text, never the
    // Android-overlay selection highlight (separate surface layer). Flip
    // LOUPE_ENABLED to restore. See docs/GPUI_ANDROID_TEXT_SELECTION.md.
    private val magnifier =
        if (LOUPE_ENABLED && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            Magnifier(surfaceView)
        } else {
            null
        }

    init {
        rootView.addView(
            overlay,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT,
            ),
        )
        activeController = WeakReference(this)
    }

    fun onSurfaceTouch(event: MotionEvent): Boolean {
        // Once the long press promotes to a selection, this gesture's target stays
        // pinned to the surface view (the overlay was GONE at ACTION_DOWN, so it
        // can never receive this stream). Drive extension directly so the user can
        // drag-to-extend within the same motion, like native text selection.
        if (longPressActive) {
            when (event.actionMasked) {
                MotionEvent.ACTION_MOVE -> extendLongPressSelection(event.x, event.y)
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    longPressActive = false
                    endHandleDrag()
                }
            }
            return true
        }
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                cancelPendingLongPress()
                downX = event.x
                downY = event.y
                pendingLongPress =
                    Runnable {
                        pendingLongPress = null
                        startSelectionFromLongPress(downX, downY)
                    }.also { handler.postDelayed(it, longPressTimeout) }
            }
            MotionEvent.ACTION_MOVE -> {
                if (hypot(event.x - downX, event.y - downY) > touchSlop) {
                    cancelPendingLongPress()
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> cancelPendingLongPress()
        }
        return false
    }

    private fun startSelectionFromLongPress(x: Float, y: Float) {
        val index = nativeSelectionStartAt(x, y)
        Log.i(GEOMETRY_TAG, "long_press surface=($x,$y) index=$index")
        if (index < 0) return
        // Granularity is owned here, not in GPUI: expand the hit index to its word
        // (BreakIterator), falling back to a single character on whitespace.
        val word = wordBoundsAt(index) ?: (index to index + 1)
        if (!nativeSelectionSetRange(word.first, word.second)) return
        longPressActive = true
        surfaceView.cancelGpuiTouchForSelection()
        refresh()
        snapshot?.let {
            longPressAnchorStart = it.start
            longPressAnchorEnd = it.end
        }
    }

    private fun extendLongPressSelection(x: Float, y: Float) {
        val anchorStart = longPressAnchorStart
        val anchorEnd = longPressAnchorEnd
        if (anchorStart < 0 || anchorEnd < 0) return
        val current = snapshot ?: return
        val index = nativeSelectionNearestIndexAt(x, y)
        if (index < 0) return
        // Keep the long-pressed word selected and grow toward the finger, snapping
        // the moving edge to words while expanding (character while contracting).
        val changed =
            when {
                index >= anchorEnd ->
                    nativeSelectionSetRange(anchorStart, snapEndpoint(index, current.end, false))
                index <= anchorStart ->
                    nativeSelectionSetRange(snapEndpoint(index, current.start, true), anchorEnd)
                else -> false
            }
        if (changed) {
            refresh()
            actionMode?.hide(ActionMode.DEFAULT_HIDE_DURATION.toLong())
        }
    }

    fun refresh() {
        val next = nativeSelectionSnapshot()?.let(SelectionSnapshot::fromNative)
        if (next == null) {
            dismiss(clearGpui = false)
            return
        }
        snapshot = next
        if (next != lastLoggedSnapshot) {
            lastLoggedSnapshot = next
            Log.i(GEOMETRY_TAG, "snapshot $next")
        }
        overlay.show(next)
        if (actionMode == null) {
            actionMode = surfaceView.startActionMode(actionModeCallback, ActionMode.TYPE_FLOATING)
        } else {
            val nextActions = nativeSelectionActions()
            if (nextActions != menuActions) {
                actionMode?.invalidate()
            } else {
                actionMode?.invalidateContentRect()
            }
        }
    }

    fun dismiss(clearGpui: Boolean) {
        cancelPendingLongPress()
        longPressActive = false
        longPressAnchorStart = -1
        longPressAnchorEnd = -1
        snapshot = null
        menuActions = emptyList()
        lastLoggedSnapshot = null
        magnifier?.dismiss()
        overlay.hide()
        actionMode?.finish()
        actionMode = null
        if (clearGpui) {
            nativeSelectionClear()
        }
    }

    fun destroy() {
        dismiss(clearGpui = true)
        // Remove the overlay this controller added so a reused root never keeps a
        // stale overlay (and its references to this controller and the surface).
        rootView.removeView(overlay)
        if (activeController?.get() === this) {
            activeController = null
        }
    }

    fun beginHandleDrag(x: Float, y: Float): DragHandle? {
        val current = snapshot ?: return null
        // Hit-test the whole handle glyph as a vertical capsule (text line down
        // through the drawn circle), not just the circle center, so taps along
        // the stem or near the baseline still grab. Pick the nearer when both hit.
        val startDistance = handleDistance(current.startHandle, start = true, x, y)
        val endDistance = handleDistance(current.endHandle, start = false, x, y)
        val handle =
            when {
                startDistance <= handleTouchRadius && startDistance <= endDistance -> DragHandle.Start
                endDistance <= handleTouchRadius -> DragHandle.End
                else -> return null
            }
        // Offset the finger to the handle's text line so dragging extends by the
        // movement delta, not the absolute touch point (which sits below the
        // baseline and would otherwise snap to the next line immediately).
        val anchor =
            when (handle) {
                DragHandle.Start -> PointF(current.startHandle.left, current.startHandle.centerY())
                DragHandle.End -> PointF(current.endHandle.right, current.endHandle.centerY())
            }
        dragOffsetX = anchor.x - x
        dragOffsetY = anchor.y - y
        return handle
    }

    // Distance from the touch to the handle modeled as a vertical capsule: a
    // segment from the glyph top down to the drawn circle's bottom, at the
    // handle's edge x. Points alongside the segment measure only horizontal
    // distance, so the entire visible handle is grabbable.
    private fun handleDistance(rect: RectF, start: Boolean, x: Float, y: Float): Float {
        val edgeX = if (start) rect.left else rect.right
        val top = rect.top
        val bottom = rect.bottom + 2f * handleRadius
        val dy =
            when {
                y < top -> y - top
                y > bottom -> y - bottom
                else -> 0f
            }
        return hypot(x - edgeX, dy)
    }

    fun dragHandle(handle: DragHandle, x: Float, y: Float) {
        val effectiveX = x + dragOffsetX
        val effectiveY = y + dragOffsetY
        val index = nativeSelectionNearestIndexAt(effectiveX, effectiveY)
        val current = snapshot
        if (index >= 0 && current != null) {
            val changed =
                when (handle) {
                    DragHandle.Start ->
                        nativeSelectionSetRange(snapEndpoint(index, current.start, true), current.end)
                    DragHandle.End ->
                        nativeSelectionSetRange(current.start, snapEndpoint(index, current.end, false))
                }
            if (changed) refresh()
        }
        // Keep the floating toolbar out of the way and center the loupe on the
        // (snapped) handle instead of the raw finger position.
        actionMode?.hide(ActionMode.DEFAULT_HIDE_DURATION.toLong())
        showMagnifierAtHandle(handle)
    }

    // Word-snap the moving endpoint while expanding away from the anchor; keep
    // character granularity while contracting. Word boundaries come from the
    // platform's BreakIterator, the Android peer of UIKit's tokenizer, so
    // granularity is owned natively and GPUI stays neutral.
    private fun snapEndpoint(rawIndex: Int, currentMoving: Int, movingStart: Boolean): Int {
        val expanding = if (movingStart) rawIndex < currentMoving else rawIndex > currentMoving
        if (!expanding) return rawIndex
        val word = wordBoundsAt(rawIndex) ?: return rawIndex
        return if (movingStart) word.first else word.second
    }

    // Returns [start, end) of the word containing [index], or null on whitespace
    // or when no document text is available. Fetches a bounded text window around
    // the index through the neutral text bridge and runs BreakIterator over it.
    private fun wordBoundsAt(index: Int): Pair<Int, Int>? {
        val windowStart = maxOf(0, index - WORD_WINDOW)
        val text = nativeSelectionTextForRange(windowStart, index + WORD_WINDOW) ?: return null
        if (text.isEmpty()) return null
        val rel = (index - windowStart).coerceIn(0, text.length - 1)
        val iterator = BreakIterator.getWordInstance()
        iterator.setText(text)
        val end = iterator.following(rel).let { if (it == BreakIterator.DONE) text.length else it }
        val start = iterator.preceding(end).let { if (it == BreakIterator.DONE) 0 else it }
        if (start >= end) return null
        if ((start until end).none { text[it].isLetterOrDigit() }) return null
        return (windowStart + start) to (windowStart + end)
    }

    private fun showMagnifierAtHandle(handle: DragHandle) {
        val current = snapshot ?: return
        val rect = if (handle == DragHandle.Start) current.startHandle else current.endHandle
        val centerX = if (handle == DragHandle.Start) rect.left else rect.right
        magnifier?.show(centerX, rect.centerY())
    }

    fun endHandleDrag() {
        magnifier?.dismiss()
        // hide(0) cancels the hide timer and brings the toolbar back immediately.
        actionMode?.hide(0L)
    }

    private fun cancelPendingLongPress() {
        pendingLongPress?.let(handler::removeCallbacks)
        pendingLongPress = null
    }

    private fun nativeSelectionActions(): List<NativeSelectionAction> {
        val count = nativeSelectionActionCount()
        if (count <= 0) {
            return emptyList()
        }
        val actions = ArrayList<NativeSelectionAction>(count)
        for (index in 0 until count) {
            val title = nativeSelectionActionTitle(index)?.trim().orEmpty()
            if (title.isNotEmpty()) {
                actions.add(NativeSelectionAction(index, title))
            }
        }
        return actions
    }

    private fun addMenuItem(menu: Menu, itemId: Int, title: CharSequence): MenuItem {
        return menu.add(Menu.NONE, itemId, Menu.NONE, title)
    }

    private fun shareSelectedText(): Boolean {
        val selectedText = nativeSelectionText() ?: return false
        val context = surfaceView.context
        val shareIntent =
            Intent(Intent.ACTION_SEND).apply {
                type = "text/plain"
                putExtra(Intent.EXTRA_TEXT, selectedText)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        return try {
            context.startActivity(
                Intent.createChooser(shareIntent, "Share").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            true
        } catch (_: ActivityNotFoundException) {
            false
        } catch (_: RuntimeException) {
            false
        }
    }

    private fun searchSelectedText(): Boolean {
        val selectedText = nativeSelectionText() ?: return false
        val context = surfaceView.context
        val searchIntent =
            Intent(Intent.ACTION_WEB_SEARCH).apply {
                putExtra(SearchManager.QUERY, selectedText)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        return try {
            context.startActivity(searchIntent)
            true
        } catch (_: ActivityNotFoundException) {
            try {
                val uri =
                    Uri.parse("https://www.google.com/search?q=" + Uri.encode(selectedText))
                context.startActivity(
                    Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
                true
            } catch (_: RuntimeException) {
                false
            }
        } catch (_: RuntimeException) {
            false
        }
    }

    private fun rebuildActionModeMenu(menu: Menu) {
        val actions = nativeSelectionActions()
        menuActions = actions
        menu.clear()
        addMenuItem(menu, MENU_COPY, "Copy")
            .setIcon(android.R.drawable.ic_menu_edit)
            .setShowAsActionFlags(MenuItem.SHOW_AS_ACTION_ALWAYS)
        addMenuItem(menu, MENU_SHARE, "Share")
            .setIcon(android.R.drawable.ic_menu_share)
            .setShowAsActionFlags(MenuItem.SHOW_AS_ACTION_ALWAYS)
        addMenuItem(menu, MENU_SEARCH, "Search")
            .setIcon(android.R.drawable.ic_menu_search)
            .setShowAsActionFlags(MenuItem.SHOW_AS_ACTION_ALWAYS)
        for (action in actions) {
            addMenuItem(menu, MENU_CUSTOM_ACTION_BASE + action.index, action.title)
                .setShowAsActionFlags(MenuItem.SHOW_AS_ACTION_NEVER)
        }
    }

    private val actionModeCallback =
        object : ActionMode.Callback2() {
            override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
                rebuildActionModeMenu(menu)
                return true
            }

            override fun onPrepareActionMode(mode: ActionMode, menu: Menu): Boolean {
                rebuildActionModeMenu(menu)
                return true
            }

            override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean {
                when (item.itemId) {
                    MENU_COPY -> {
                        if (nativeSelectionCopy()) {
                            dismiss(clearGpui = true)
                        }
                        return true
                    }

                    MENU_SHARE -> {
                        if (shareSelectedText()) {
                            dismiss(clearGpui = true)
                        }
                        return true
                    }

                    MENU_SEARCH -> {
                        if (searchSelectedText()) {
                            dismiss(clearGpui = true)
                        }
                        return true
                    }

                }

                if (item.itemId >= MENU_CUSTOM_ACTION_BASE) {
                    val actionIndex = item.itemId - MENU_CUSTOM_ACTION_BASE
                    if (nativeSelectionPerformAction(actionIndex)) {
                        dismiss(clearGpui = true)
                    }
                    return true
                }

                return false
            }

            override fun onDestroyActionMode(mode: ActionMode) {
                if (actionMode === mode) actionMode = null
            }

            override fun onGetContentRect(mode: ActionMode, view: View, outRect: Rect) {
                val bounds = snapshot?.contentBounds ?: return
                bounds.roundOut(outRect)
            }
        }

    internal enum class DragHandle {
        Start,
        End,
    }

    companion object {
        internal const val GEOMETRY_TAG = "ZEDRA_SELECTION_GEOMETRY"
        // Loupe disabled until the canvas/overlay share one surface (TextureView).
        private const val LOUPE_ENABLED = false
        // UTF-16 units fetched on each side of the hit index for word lookup.
        private const val WORD_WINDOW = 96
        private var activeController: WeakReference<SelectionController>? = null

        @JvmStatic
        fun refreshActiveSelection() {
            activeController?.get()?.surfaceView?.post {
                activeController?.get()?.refresh()
            }
        }

        @JvmStatic
        fun dismissActiveSelection() {
            activeController?.get()?.surfaceView?.post {
                activeController?.get()?.dismiss(clearGpui = false)
            }
        }

        @JvmStatic private external fun nativeSelectionStartAt(x: Float, y: Float): Int

        @JvmStatic private external fun nativeSelectionNearestIndexAt(x: Float, y: Float): Int

        @JvmStatic private external fun nativeSelectionSetRange(start: Int, end: Int): Boolean

        @JvmStatic private external fun nativeSelectionTextForRange(start: Int, end: Int): String?

        @JvmStatic private external fun nativeSelectionSnapshot(): DoubleArray?

        @JvmStatic private external fun nativeSelectionCopy(): Boolean
        @JvmStatic private external fun nativeSelectionText(): String?
        @JvmStatic private external fun nativeSelectionActionCount(): Int
        @JvmStatic private external fun nativeSelectionActionTitle(index: Int): String?
        @JvmStatic private external fun nativeSelectionPerformAction(index: Int): Boolean

        @JvmStatic private external fun nativeSelectionClear()

        private const val MENU_COPY = 1
        private const val MENU_SHARE = 2
        private const val MENU_SEARCH = 3
        private const val MENU_CUSTOM_ACTION_BASE = 1000
    }
}

private data class SelectionSnapshot(
    val start: Int,
    val end: Int,
    val rects: List<RectF>,
    val startHandle: RectF,
    val endHandle: RectF,
) {
    // Only read when the action mode repaints its content rect, so compute lazily
    // rather than on every per-frame snapshot.
    val contentBounds: RectF by lazy {
        rects.drop(1).fold(rects.firstOrNull()?.let(::RectF) ?: RectF(startHandle)) { bounds, rect ->
            bounds.apply { union(rect) }
        }.apply { union(endHandle) }
    }

    companion object {
        fun fromNative(values: DoubleArray): SelectionSnapshot? {
            if (values.size < 12) return null
            val rectCount = values[3].toInt()
            val expectedSize = 4 + rectCount * 4 + 8
            if (rectCount < 0 || values.size < expectedSize) return null
            fun rectAt(offset: Int) =
                RectF(
                    values[offset].toFloat(),
                    values[offset + 1].toFloat(),
                    (values[offset] + values[offset + 2]).toFloat(),
                    (values[offset + 1] + values[offset + 3]).toFloat(),
                )
            val rects = List(rectCount) { index -> rectAt(4 + index * 4) }
            val handlesOffset = 4 + rectCount * 4
            val startHandle = rectAt(handlesOffset)
            val endHandle = rectAt(handlesOffset + 4)
            return SelectionSnapshot(values[0].toInt(), values[1].toInt(), rects, startHandle, endHandle)
        }
    }
}

private class SelectionOverlayView(
    surfaceView: View,
    private val controller: SelectionController,
) : View(surfaceView.context) {
    private val surfaceView = surfaceView
    private val density = resources.displayMetrics.density
    private val handleRadius = 10f * density
    private val highlightPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val handlePaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private var snapshot: SelectionSnapshot? = null
    private var dragging: SelectionController.DragHandle? = null
    private var coordinateSpace = SelectionOverlayCoordinateSpace()
    private var lastLoggedGeometry: Pair<SelectionSnapshot, SelectionOverlayCoordinateSpace>? = null

    init {
        visibility = GONE
        isClickable = true
        highlightPaint.color = selectionHighlightColor()
        handlePaint.color = themedColor(android.R.attr.colorAccent, 0xFF2196F3.toInt())
    }

    fun show(snapshot: SelectionSnapshot) {
        this.snapshot = snapshot
        coordinateSpace = SelectionOverlayCoordinateSpace.between(this, surfaceView)
        val geometry = snapshot to coordinateSpace
        if (geometry != lastLoggedGeometry) {
            lastLoggedGeometry = geometry
            Log.i(
                SelectionController.GEOMETRY_TAG,
                "overlay_show overlay=${geometryDescription()} surface=${surfaceView.geometryDescription()} coordinates=$coordinateSpace",
            )
        }
        visibility = VISIBLE
        invalidate()
    }

    fun hide() {
        dragging = null
        snapshot = null
        lastLoggedGeometry = null
        visibility = GONE
    }

    override fun onDraw(canvas: Canvas) {
        val current = snapshot ?: return
        canvas.save()
        canvas.translate(coordinateSpace.surfaceOriginX, coordinateSpace.surfaceOriginY)
        for (rect in current.rects) {
            canvas.drawRect(rect, highlightPaint)
        }
        drawHandle(canvas, current.startHandle, start = true)
        drawHandle(canvas, current.endHandle, start = false)
        canvas.restore()
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val surfaceX = coordinateSpace.overlayToSurfaceX(event.x)
        val surfaceY = coordinateSpace.overlayToSurfaceY(event.y)
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                dragging = controller.beginHandleDrag(surfaceX, surfaceY)
                Log.i(
                    SelectionController.GEOMETRY_TAG,
                    "overlay_down surface=($surfaceX,$surfaceY) grabbed=$dragging",
                )
                if (dragging == null) {
                    controller.dismiss(clearGpui = true)
                }
            }
            MotionEvent.ACTION_MOVE -> dragging?.let { controller.dragHandle(it, surfaceX, surfaceY) }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                dragging = null
                controller.endHandleDrag()
            }
        }
        return true
    }

    private fun drawHandle(canvas: Canvas, bounds: RectF, start: Boolean) {
        val x = if (start) bounds.left else bounds.right
        val y = bounds.bottom
        canvas.drawCircle(x, y + handleRadius, handleRadius, handlePaint)
        canvas.drawRect(x - density, y, x + density, y + handleRadius, handlePaint)
    }

    private fun selectionHighlightColor(): Int {
        val fallback = 0x6633B5E5
        val resolved = themedColor(android.R.attr.textColorHighlight, fallback)
        // Some themes resolve textColorHighlight to a fully transparent color
        // (data=0), which draws an invisible selection; fall back to a tint.
        return if (Color.alpha(resolved) == 0) fallback else resolved
    }

    private fun themedColor(attribute: Int, fallback: Int): Int {
        val value = TypedValue()
        return if (context.theme.resolveAttribute(attribute, value, true)) value.data else fallback
    }
}

private fun View.geometryDescription(): String {
    val location = IntArray(2)
    getLocationInWindow(location)
    return "window=(${location[0]},${location[1]}) local=($left,$top)-($right,$bottom) size=${width}x$height translation=($translationX,$translationY)"
}

internal data class SelectionOverlayCoordinateSpace(
    val surfaceOriginX: Float = 0f,
    val surfaceOriginY: Float = 0f,
) {
    fun overlayToSurfaceX(x: Float): Float = x - surfaceOriginX

    fun overlayToSurfaceY(y: Float): Float = y - surfaceOriginY

    companion object {
        fun between(overlay: View, surface: View): SelectionOverlayCoordinateSpace {
            val overlayLocation = IntArray(2)
            val surfaceLocation = IntArray(2)
            overlay.getLocationInWindow(overlayLocation)
            surface.getLocationInWindow(surfaceLocation)
            return SelectionOverlayCoordinateSpace(
                surfaceOriginX = (surfaceLocation[0] - overlayLocation[0]).toFloat(),
                surfaceOriginY = (surfaceLocation[1] - overlayLocation[1]).toFloat(),
            )
        }
    }
}
