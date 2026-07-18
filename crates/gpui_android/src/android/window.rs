use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use futures::channel::oneshot;
use ndk::native_window::NativeWindow;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, HasDisplayHandle, HasWindowHandle,
    RawDisplayHandle, RawWindowHandle,
};
use smallvec::SmallVec;

use gpui::{
    AnyWindowHandle, Bounds, Capslock, DevicePixels, DispatchEventResult, GpuSpecs, Modifiers,
    Pixels, PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow,
    Point, PointerButton, PointerCancelEvent, PointerDownEvent, PointerKind, PointerMoveEvent,
    PointerUpEvent, PromptButton, PromptLevel, RequestFrameOptions, Scene, ScrollDelta,
    ScrollWheelEvent, SelectableTextHitRegion, SelectionMenuPresentation, Size, TouchPhase,
    UTF16Selection, WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
    point, px, should_auto_request_soft_keyboard,
};
use gpui_wgpu::{GpuContext, WgpuAtlas, WgpuRenderer, WgpuSurfaceConfig};

const TAP_SLOP: f32 = 4.0;
const FLING_THRESHOLD: f32 = 50.0;

const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVE: i32 = 2;
const ACTION_CANCEL: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidWindowRole {
    Root,
    EmbeddedSheet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveSelectionSource {
    Input,
    ReadOnly,
}

struct FlingState {
    velocity_x: f32,
    velocity_y: f32,
    last_time: std::time::Instant,
    position: Point<Pixels>,
}

struct TouchState {
    last_position: Option<(f32, f32)>,
    down_position: Option<(f32, f32)>,
    is_drag: bool,
    suppress_scroll: bool,
    fling: Option<FlingState>,
    /// Set after a long-press fires, cleared on pointer-up/cancel.
    long_press_active: bool,
    /// UTF-16 anchor index established when the long-press fires.
    long_press_anchor_utf16: Option<usize>,
}

impl TouchState {
    fn new() -> Self {
        Self {
            last_position: None,
            down_position: None,
            is_drag: false,
            suppress_scroll: false,
            fling: None,
            long_press_active: false,
            long_press_anchor_utf16: None,
        }
    }
}

pub(crate) struct AndroidAtlas {
    atlas: Mutex<Option<Arc<WgpuAtlas>>>,
}

impl AndroidAtlas {
    fn new() -> Self {
        Self {
            atlas: Mutex::new(None),
        }
    }

    fn bind(&self, atlas: Arc<WgpuAtlas>) {
        *self.atlas.lock().expect("AndroidAtlas poisoned") = Some(atlas);
    }

    fn gpu_atlas(&self) -> Option<Arc<WgpuAtlas>> {
        self.atlas.lock().expect("AndroidAtlas poisoned").clone()
    }
}

impl PlatformAtlas for AndroidAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &gpui::AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>>,
    ) -> Result<Option<gpui::AtlasTile>> {
        if let Some(atlas) = self.gpu_atlas() {
            atlas.get_or_insert_with(key, build)
        } else {
            Ok(None)
        }
    }

    fn remove(&self, key: &gpui::AtlasKey) {
        if let Some(atlas) = self.gpu_atlas() {
            atlas.remove(key);
        }
    }
}

#[derive(Default)]
pub(crate) struct Callbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    hover_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    close: Option<Box<dyn FnOnce()>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
    hit_test_window_control: Option<Box<dyn FnMut() -> Option<WindowControlArea>>>,
}

#[derive(Clone, Copy, Debug)]
struct RawWindow {
    window: *mut c_void,
}

unsafe impl Send for RawWindow {}
unsafe impl Sync for RawWindow {}

impl HasWindowHandle for RawWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let window =
            NonNull::new(self.window).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = AndroidNdkWindowHandle::new(window.cast());
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(RawWindowHandle::AndroidNdk(handle))
        })
    }
}

impl HasDisplayHandle for RawWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        let handle = AndroidDisplayHandle::new();
        Ok(unsafe {
            raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Android(handle))
        })
    }
}

type AndroidRenderer = super::pipelined_renderer::PipelinedRenderer;

pub struct AndroidWindowState {
    role: AndroidWindowRole,
    // Opaque, stable per-window id used to route native text selection to the
    // right GPUI window independent of role (0 = root/primary window). Survives
    // surface re-creation so a re-shown sheet keeps the same selection target.
    window_handle: u64,
    raw_window: Option<RawWindow>,
    native_window: Option<NativeWindow>,
    renderer: Option<AndroidRenderer>,
    atlas: Arc<AndroidAtlas>,
    bounds: Bounds<Pixels>,
    scale: f32,
    touch_state: TouchState,
    input_handler: Option<PlatformInputHandler>,
    selection_handler: Option<PlatformInputHandler>,
    selectable_text_hit_regions: SmallVec<[SelectableTextHitRegion; 8]>,
    active_selection_source: Option<ActiveSelectionSource>,
    // Cached from the last set_input_handler. Window::draw takes the input
    // handler for the duration of the frame, so views rendering mid-draw must
    // read this flag instead of querying the (absent) live handler.
    input_keyboard_accessory: bool,
    callbacks: Callbacks,
    active: bool,
    appearance: WindowAppearance,
    background_appearance: WindowBackgroundAppearance,
    modifiers: Modifiers,
    last_mouse_position: Point<Pixels>,
    subpixel_supported: Option<bool>,
}

impl AndroidWindowState {
    pub fn new(
        role: AndroidWindowRole,
        window_handle: u64,
        bounds: Bounds<Pixels>,
        scale: f32,
        active: bool,
    ) -> Self {
        Self {
            role,
            window_handle,
            raw_window: None,
            native_window: None,
            renderer: None,
            atlas: Arc::new(AndroidAtlas::new()),
            bounds,
            scale,
            touch_state: TouchState::new(),
            input_handler: None,
            selection_handler: None,
            selectable_text_hit_regions: SmallVec::new(),
            active_selection_source: None,
            input_keyboard_accessory: false,
            callbacks: Callbacks::default(),
            active,
            appearance: WindowAppearance::Light,
            // See: docs/GPUI_ANDROID_PERFORMANCE.md § opaque-window-default
            background_appearance: WindowBackgroundAppearance::Opaque,
            modifiers: Modifiers::default(),
            last_mouse_position: Point::default(),
            subpixel_supported: None,
        }
    }

    pub fn role(&self) -> AndroidWindowRole {
        self.role
    }

    pub fn window_handle(&self) -> u64 {
        self.window_handle
    }

    pub fn set_active(&mut self, active: bool) -> bool {
        if self.active == active {
            false
        } else {
            self.active = active;
            true
        }
    }

    pub fn take_active_status_change_callback(&mut self) -> Option<Box<dyn FnMut(bool)>> {
        self.callbacks.active_status_change.take()
    }

    pub fn restore_active_status_change_callback(&mut self, callback: Box<dyn FnMut(bool)>) {
        self.callbacks.active_status_change = Some(callback);
    }

    pub fn handle_surface_created(
        &mut self,
        native_window: NativeWindow,
        gpu_context: GpuContext,
    ) -> Result<()> {
        log::info!("AndroidWindow({:?}): surface created", self.role);

        let window_ptr = native_window.ptr().as_ptr() as *mut c_void;
        let raw_window = RawWindow { window: window_ptr };

        let config = self.surface_config(self.physical_size());

        log::info!(
            "Creating WgpuRenderer with physical size: {}x{} (logical: {}x{}, scale: {})",
            config.size.width.0,
            config.size.height.0,
            f32::from(self.bounds.size.width),
            f32::from(self.bounds.size.height),
            self.scale
        );

        if let Some(renderer) = self.renderer.as_mut() {
            // Keep the presentation mode selected during initial surface
            // creation; `replace_surface` does not re-query surface
            // capabilities, so passing `None` preserves the known-good mode.
            let mut config = config;
            config.preferred_present_mode = None;
            let context = gpu_context.borrow();
            let context = context
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Cannot replace Android surface before GPU init"))?;
            renderer
                .lock()
                .replace_surface(&raw_window, config, &context.instance)?;
        } else {
            let renderer = if let Some(atlas) = self.atlas.gpu_atlas() {
                WgpuRenderer::new_with_atlas(gpu_context, &raw_window, config, None, atlas)?
            } else {
                WgpuRenderer::new(gpu_context, &raw_window, config, None)?
            };
            self.atlas.bind(renderer.atlas());
            let supports_subpixel = renderer.supports_dual_source_blending();
            self.subpixel_supported = Some(supports_subpixel);
            {
                self.renderer = Some(super::pipelined_renderer::PipelinedRenderer::new(renderer));
            }
        }

        self.native_window = Some(native_window);
        self.raw_window = Some(raw_window);

        Ok(())
    }

    pub fn handle_surface_changed(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<(Size<Pixels>, f32)>> {
        log::info!(
            "AndroidWindow({:?}): surface changed to {}x{}",
            self.role,
            width,
            height
        );

        let new_bounds = Bounds {
            origin: self.bounds.origin,
            size: Size {
                width: gpui::px(width as f32 / self.scale),
                height: gpui::px(height as f32 / self.scale),
            },
        };

        let bounds_changed = new_bounds != self.bounds;
        if bounds_changed {
            self.bounds = new_bounds;
        }

        if let Some(renderer) = self.renderer.as_mut() {
            let size = Size {
                width: DevicePixels(width as i32),
                height: DevicePixels(height as i32),
            };
            renderer.lock().update_drawable_size(size);
        }

        let has_callback = self.callbacks.resize.is_some();
        Ok(if has_callback {
            Some((new_bounds.size, self.scale))
        } else {
            None
        })
    }

    pub fn take_resize_callback(&mut self) -> Option<Box<dyn FnMut(Size<Pixels>, f32)>> {
        self.callbacks.resize.take()
    }

    pub fn restore_resize_callback(&mut self, callback: Option<Box<dyn FnMut(Size<Pixels>, f32)>>) {
        self.callbacks.resize = callback;
    }

    pub fn take_input_callback(
        &mut self,
    ) -> Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>> {
        self.callbacks.input.take()
    }

    pub fn restore_input_callback(
        &mut self,
        callback: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    ) {
        self.callbacks.input = callback;
    }

    pub fn handle_surface_destroyed(&mut self) {
        log::info!("AndroidWindow({:?}): surface destroyed", self.role);

        if let Some(renderer) = self.renderer.as_mut() {
            renderer.lock().unconfigure_surface();
        }
        self.native_window = None;
        self.raw_window = None;
    }

    pub fn draw(&mut self, scene: &Scene) {
        if let Some(ref renderer) = self.renderer {
            // Device-lost recovery requires `raw_window` and mutable
            // access to the renderer. Check briefly under the lock; if
            // lost, recover synchronously, then dispatch the draw async.
            {
                let mut r = renderer.lock();
                if r.device_lost() {
                    if let Some(raw_window) = self.raw_window {
                        if let Err(error) = r.recover(&raw_window) {
                            log::error!("Failed to recover Android renderer: {error:?}");
                            return;
                        }
                    } else {
                        return;
                    }
                }
            }
            renderer.draw(scene);
        }
    }

    fn physical_size(&self) -> Size<DevicePixels> {
        Size {
            width: DevicePixels((f32::from(self.bounds.size.width) * self.scale) as i32),
            height: DevicePixels((f32::from(self.bounds.size.height) * self.scale) as i32),
        }
    }

    fn surface_config(&self, size: Size<DevicePixels>) -> WgpuSurfaceConfig {
        WgpuSurfaceConfig {
            size,
            transparent: matches!(
                self.background_appearance,
                WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred
            ),
            // See: docs/GPUI_ANDROID_PERFORMANCE.md § present-fifo
            preferred_present_mode: Some(gpui_wgpu::wgpu::PresentMode::Fifo),
        }
    }

    pub fn request_frame(&mut self) {
        if let Some(ref mut callback) = self.callbacks.request_frame {
            callback(RequestFrameOptions::default());
        }
    }

    pub(crate) fn take_request_frame_callback(
        &mut self,
    ) -> Option<Box<dyn FnMut(RequestFrameOptions)>> {
        self.callbacks.request_frame.take()
    }

    pub(crate) fn put_request_frame_callback(
        &mut self,
        callback: Box<dyn FnMut(RequestFrameOptions)>,
    ) {
        self.callbacks.request_frame = Some(callback);
    }

    pub fn handle_input(&mut self, input: PlatformInput) -> DispatchEventResult {
        if let Some(ref mut callback) = self.callbacks.input {
            callback(input)
        } else {
            DispatchEventResult::default()
        }
    }

    fn with_text_input_handler(&mut self, f: impl FnOnce(&mut PlatformInputHandler)) -> bool {
        let Some(mut input_handler) = self.input_handler.take() else {
            return false;
        };

        let handled = if input_handler.query_accepts_text_input() {
            f(&mut input_handler);
            true
        } else {
            false
        };
        self.input_handler = Some(input_handler);
        handled
    }

    fn with_selection_source_handler<R>(
        &mut self,
        source: ActiveSelectionSource,
        f: impl FnOnce(&mut PlatformInputHandler) -> R,
    ) -> Option<R> {
        let slot = match source {
            ActiveSelectionSource::Input => &mut self.input_handler,
            ActiveSelectionSource::ReadOnly => &mut self.selection_handler,
        };
        let mut handler = slot.take()?;
        let result = f(&mut handler);
        *slot = Some(handler);
        Some(result)
    }

    fn with_active_selection_handler<R>(
        &mut self,
        f: impl FnOnce(&mut PlatformInputHandler) -> R,
    ) -> Option<R> {
        self.with_selection_source_handler(self.active_selection_source?, f)
    }

    /// Hit-test a long press and establish the active selection source, returning
    /// the UTF-16 index under the point. Granularity (word vs character) is owned
    /// by the native presenter (SelectionController), which then commits a range
    /// through `update_active_selection`. iOS owns granularity via UIKit's
    /// tokenizer; Android via `BreakIterator`. GPUI stays neutral.
    pub fn start_selection_at(&mut self, physical_x: f32, physical_y: f32) -> Option<usize> {
        let point = point(px(physical_x / self.scale), px(physical_y / self.scale));
        let input_index = self.input_handler.as_mut().and_then(|handler| {
            handler
                .query_handles_native_selection()
                .then(|| handler.character_index_for_point(point))
                .flatten()
        });
        let source_and_index = input_index
            .map(|index| (ActiveSelectionSource::Input, index))
            .or_else(|| {
                self.selectable_text_hit_regions
                    .iter()
                    .any(|region| region.contains_text(point))
                    .then(|| {
                        self.selection_handler
                            .as_mut()?
                            .character_index_for_point(point)
                            .map(|index| (ActiveSelectionSource::ReadOnly, index))
                    })
                    .flatten()
            });
        let Some((source, index)) = source_and_index else {
            return None;
        };
        self.active_selection_source = Some(source);
        Some(index)
    }

    /// Return the document text for a UTF-16 range from the active selection
    /// source so the native presenter can compute word boundaries.
    pub fn selection_text_for_range(&mut self, start: usize, end: usize) -> Option<String> {
        self.with_active_selection_handler(|handler| {
            let mut adjusted = None;
            handler.text_for_range(start..end, &mut adjusted)
        })
        .flatten()
    }

    /// Commit a selection range chosen by the native presenter. Neutral: clamp via
    /// the handler and set it; no word/granularity policy lives here.
    pub fn update_active_selection(&mut self, start: usize, end: usize) -> bool {
        self.with_active_selection_handler(|handler| {
            let requested = start.min(end)..start.max(end);
            let adjusted = handler
                .adjusted_native_selection_range(requested.clone())
                .unwrap_or(requested);
            if adjusted.is_empty() {
                return false;
            }
            let current = handler.selected_text_range(false).map(|s| s.range);
            if current.as_ref() == Some(&adjusted) {
                return false;
            }
            // Refuse an initial selection that renders nothing (e.g. a long press
            // landing on a separator/blank line between paragraphs): a phantom with
            // no visible geometry would leave the document stuck. Only the first
            // commit can be non-visible — extending an existing (non-empty)
            // selection always includes visible content — so skip this probe on the
            // per-drag path. `selected_text_range` reports `0..0` (empty), never
            // `None`, when there is no selection, so test emptiness, not presence.
            let extending = current.is_some_and(|range| !range.is_empty());
            if !extending && handler.rects_for_range(adjusted.clone()).is_empty() {
                return false;
            }
            handler.set_selected_text_range(adjusted.clone());
            true
        })
        .unwrap_or(false)
    }

    pub fn nearest_selection_index(&mut self, physical_x: f32, physical_y: f32) -> Option<usize> {
        let point = point(px(physical_x / self.scale), px(physical_y / self.scale));
        self.with_active_selection_handler(|handler| {
            handler.nearest_character_index_for_point(point)
        })
        .flatten()
    }

    pub fn active_selection_snapshot(&mut self) -> Option<Vec<f64>> {
        let scale = self.scale;
        let snapshot = self
            .with_active_selection_handler(|handler| {
                let UTF16Selection { range, reversed } = handler.selected_text_range(false)?;
                if range.is_empty() {
                    return None;
                }
                let rects = handler.rects_for_range(range.clone());
                // A non-empty range that produces no rects covers only
                // non-visible content (a separator/newline, e.g. a long press on
                // empty space). It has no highlight, so present nothing rather
                // than a stray handle anchored to a fallback caret.
                if rects.is_empty() {
                    return None;
                }
                // Anchor the handles to the selection rects, which are always a
                // single text line tall and share the highlight's geometry.
                let line_caret = |bounds: Bounds<Pixels>, at_end: bool| {
                    let x = if at_end {
                        bounds.origin.x + bounds.size.width
                    } else {
                        bounds.origin.x
                    };
                    Bounds::new(
                        gpui::point(x, bounds.origin.y),
                        gpui::size(gpui::px(0.0), bounds.size.height),
                    )
                };
                let start_bounds = rects
                    .first()
                    .map(|rect| line_caret(*rect, false))
                    .or_else(|| handler.bounds_for_range(range.start..range.start));
                let end_bounds = rects
                    .last()
                    .map(|rect| line_caret(*rect, true))
                    .or_else(|| handler.bounds_for_range(range.end..range.end));
                let scaled = |bounds: Bounds<Pixels>| {
                    [
                        (bounds.origin.x.as_f32() * scale) as f64,
                        (bounds.origin.y.as_f32() * scale) as f64,
                        (bounds.size.width.as_f32() * scale) as f64,
                        (bounds.size.height.as_f32() * scale) as f64,
                    ]
                };
                let mut snapshot = Vec::with_capacity(4 + rects.len() * 4 + 8);
                snapshot.extend([
                    range.start as f64,
                    range.end as f64,
                    if reversed { 1.0 } else { 0.0 },
                    rects.len() as f64,
                ]);
                for rect in rects {
                    snapshot.extend(scaled(rect));
                }
                for bounds in [start_bounds, end_bounds] {
                    snapshot.extend(scaled(bounds.unwrap_or_default()));
                }
                Some(snapshot)
            })
            .flatten();
        snapshot
    }

    pub fn selected_text(&mut self) -> Option<String> {
        self.with_active_selection_handler(|handler| {
            let range = handler.selected_text_range(false)?.range;
            handler.text_for_range(range, &mut None)
        })
        .flatten()
    }

    pub fn selection_action_count(&mut self) -> usize {
        self.with_active_selection_handler(|handler| handler.selection_action_names().len())
            .unwrap_or(0)
    }

    pub fn selection_custom_actions_only(&mut self) -> bool {
        self.with_active_selection_handler(|handler| {
            handler.selection_menu_presentation() == SelectionMenuPresentation::CustomActionsOnly
        })
        .unwrap_or(false)
    }

    pub fn selection_action_title(&mut self, action_index: usize) -> Option<String> {
        self.with_active_selection_handler(|handler| {
            handler.selection_action_names().get(action_index).cloned()
        })
        .flatten()
    }

    pub fn perform_selection_action(&mut self, action_index: usize) -> bool {
        self.with_active_selection_handler(|handler| {
            if action_index < handler.selection_action_names().len() {
                handler.perform_selection_action(action_index);
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
    }

    pub fn clear_active_selection(&mut self, clear_handler: bool) {
        if clear_handler {
            self.with_active_selection_handler(|handler| handler.clear_selected_text_range());
        }
        self.active_selection_source = None;
    }

    pub fn insert_text(&mut self, text: &str) -> bool {
        let Some(mut input_handler) = self.input_handler.take() else {
            return false;
        };

        let handled = if input_handler.query_accepts_text_input() {
            // Android commitText is already confirmed input. Mirror iOS'
            // shouldChangeText preflight so terminal typing does not look like
            // UIKit's no-preflight dictation stream.
            if input_handler.should_change_text_in_range(None, text) {
                input_handler.insert_text(text);
            }
            true
        } else {
            false
        };
        self.input_handler = Some(input_handler);
        handled
    }

    pub fn delete_backward(&mut self, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        self.with_text_input_handler(|handler| {
            for _ in 0..count {
                handler.delete_backward();
            }
        })
    }

    pub fn set_composing_text(&mut self, text: &str, new_cursor_position: i32) -> bool {
        let len = text.encode_utf16().count();
        let cursor = if new_cursor_position > 0 {
            len.saturating_add((new_cursor_position - 1) as usize)
        } else {
            new_cursor_position.saturating_neg() as usize
        }
        .min(len);

        self.with_text_input_handler(|handler| {
            if text.is_empty() {
                handler.unmark_text();
            } else {
                handler.set_marked_text(text, Some(cursor..cursor), None);
            }
        })
    }

    pub fn finish_composing_text(&mut self) -> bool {
        self.with_text_input_handler(|handler| handler.unmark_text())
    }

    pub fn handle_keyboard_accessory_action(&mut self, action: &str) -> bool {
        let Some(mut input_handler) = self.input_handler.take() else {
            return false;
        };

        let handled = if input_handler.query_accepts_text_input()
            && input_handler.query_keyboard_accessory()
        {
            input_handler.handle_keyboard_accessory_action(action)
        } else {
            false
        };
        self.input_handler = Some(input_handler);
        handled
    }

    pub fn has_active_keyboard_accessory(&mut self) -> bool {
        // Fall back to the cached flag when the handler is taken (mid-draw).
        let Some(mut input_handler) = self.input_handler.take() else {
            return self.input_keyboard_accessory;
        };
        let has_accessory =
            input_handler.query_accepts_text_input() && input_handler.query_keyboard_accessory();
        self.input_handler = Some(input_handler);
        self.input_keyboard_accessory = has_accessory;
        has_accessory
    }

    /// Handle a long-press gesture at physical-pixel coordinates.
    ///
    /// Called from Kotlin's `GestureDetector.onLongPress`. Enters selection
    /// mode if the point lands on a registered selectable-text region.
    pub fn handle_long_press(&mut self, x: f32, y: f32) {
        let logical_x = x / self.scale;
        let logical_y = y / self.scale;
        let position = point(px(logical_x), px(logical_y));

        let hits_selectable = self
            .selectable_text_hit_regions
            .iter()
            .any(|region| region.contains_text(position));
        if !hits_selectable {
            return;
        }

        let Some(mut handler) = self.selection_handler.take() else {
            return;
        };
        let anchor = handler.character_index_for_point(position);
        self.selection_handler = Some(handler);

        let Some(anchor_index) = anchor else {
            return;
        };

        self.touch_state.long_press_active = true;
        self.touch_state.long_press_anchor_utf16 = Some(anchor_index);
        // Suppress scroll so the drag extends the selection instead of scrolling.
        self.touch_state.suppress_scroll = true;
    }

    /// Handle a raw Android touch event (ACTION_DOWN/MOVE/UP/CANCEL).
    ///
    /// Coordinates are physical pixels. Android windows keep touch state
    /// independently so root and embedded surfaces do not steal fling or scroll
    /// suppression state from each other.
    pub fn handle_touch(&mut self, action: i32, x: f32, y: f32) {
        let logical_x = x / self.scale;
        let logical_y = y / self.scale;
        let position = point(px(logical_x), px(logical_y));
        self.last_mouse_position = position;

        match action {
            ACTION_DOWN => {
                self.touch_state.fling = None;
                self.touch_state.down_position = Some((logical_x, logical_y));
                self.touch_state.last_position = Some((logical_x, logical_y));
                self.touch_state.is_drag = false;
                self.touch_state.suppress_scroll = false;
                self.handle_input(PlatformInput::PointerDown(PointerDownEvent {
                    pointer_id: 1,
                    kind: PointerKind::Touch,
                    is_primary: true,
                    button: PointerButton::Primary,
                    position,
                    modifiers: Modifiers::default(),
                }));
            }
            ACTION_UP => {
                let is_drag = self.touch_state.is_drag;
                let suppress_scroll = self.touch_state.suppress_scroll;
                let was_long_press = self.touch_state.long_press_active;
                self.touch_state.last_position = None;
                self.touch_state.down_position = None;
                self.touch_state.is_drag = false;
                self.touch_state.long_press_active = false;
                self.touch_state.long_press_anchor_utf16 = None;

                if !is_drag || suppress_scroll {
                    // Java forwards velocity unconditionally. If the pointer
                    // handler prevented default, the matching fling must die
                    // with the suppressed synthetic scroll stream.
                    self.touch_state.fling = None;
                } else if !was_long_press {
                    self.handle_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Ended,
                    }));
                }
                self.touch_state.suppress_scroll = false;
                self.handle_input(PlatformInput::PointerUp(PointerUpEvent {
                    pointer_id: 1,
                    kind: PointerKind::Touch,
                    is_primary: true,
                    button: PointerButton::Primary,
                    position,
                    modifiers: Modifiers::default(),
                }));
            }
            ACTION_MOVE => {
                let (scroll_delta, should_scroll) = {
                    if !self.touch_state.is_drag {
                        if let Some((down_x, down_y)) = self.touch_state.down_position {
                            let dx = logical_x - down_x;
                            let dy = logical_y - down_y;
                            if (dx * dx + dy * dy).sqrt() > TAP_SLOP {
                                self.touch_state.is_drag = true;
                            }
                        }
                    }

                    if self.touch_state.is_drag {
                        let delta = self
                            .touch_state
                            .last_position
                            .map(|(last_x, last_y)| (logical_x - last_x, logical_y - last_y));
                        self.touch_state.last_position = Some((logical_x, logical_y));
                        (delta, true)
                    } else {
                        self.touch_state.last_position = Some((logical_x, logical_y));
                        (None, false)
                    }
                };

                // When a long-press triggered selection, drag extends the range.
                if self.touch_state.long_press_active {
                    if let Some(anchor) = self.touch_state.long_press_anchor_utf16 {
                        if let Some(mut handler) = self.selection_handler.take() {
                            if let Some(current) =
                                handler.nearest_character_index_for_point(position)
                            {
                                let range = anchor.min(current)..anchor.max(current);
                                handler.set_selected_text_range(range);
                            }
                            self.selection_handler = Some(handler);
                        }
                    }
                    self.handle_input(PlatformInput::PointerMove(PointerMoveEvent {
                        pointer_id: 1,
                        kind: PointerKind::Touch,
                        is_primary: true,
                        pressed_button: Some(PointerButton::Primary),
                        position,
                        modifiers: Modifiers::default(),
                    }));
                    return;
                }

                let pointer_result =
                    self.handle_input(PlatformInput::PointerMove(PointerMoveEvent {
                        pointer_id: 1,
                        kind: PointerKind::Touch,
                        is_primary: true,
                        pressed_button: Some(PointerButton::Primary),
                        position,
                        modifiers: Modifiers::default(),
                    }));
                if pointer_result.default_prevented {
                    self.touch_state.suppress_scroll = true;
                }
                if should_scroll {
                    if let Some((dx, dy)) = scroll_delta {
                        if !self.touch_state.suppress_scroll {
                            self.handle_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
                                position,
                                delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                                modifiers: Modifiers::default(),
                                touch_phase: TouchPhase::Moved,
                            }));
                        }
                    }
                }
            }
            ACTION_CANCEL => {
                self.touch_state.last_position = None;
                self.touch_state.down_position = None;
                self.touch_state.is_drag = false;
                self.touch_state.suppress_scroll = false;
                self.touch_state.fling = None;
                self.touch_state.long_press_active = false;
                self.touch_state.long_press_anchor_utf16 = None;
                self.handle_input(PlatformInput::PointerCancel(PointerCancelEvent {
                    pointer_id: 1,
                    kind: PointerKind::Touch,
                    is_primary: true,
                    position,
                    modifiers: Modifiers::default(),
                }));
            }
            _ => {}
        }
    }

    /// Handle a fling gesture in physical pixels/second from Android VelocityTracker.
    pub fn handle_fling(&mut self, velocity_x: f32, velocity_y: f32) {
        if self.touch_state.suppress_scroll {
            self.touch_state.fling = None;
            return;
        }

        let vx = velocity_x / self.scale;
        let vy = velocity_y / self.scale;
        let position = self
            .touch_state
            .last_position
            .map(|(x, y)| point(px(x), px(y)))
            .unwrap_or_default();

        if vx.abs() > FLING_THRESHOLD || vy.abs() > FLING_THRESHOLD {
            self.touch_state.fling = Some(FlingState {
                velocity_x: vx,
                velocity_y: vy,
                last_time: std::time::Instant::now(),
                position,
            });
        }
    }

    pub fn has_active_fling(&self) -> bool {
        self.touch_state.fling.is_some()
    }

    pub fn process_fling(&mut self) {
        let Some(fling) = self.touch_state.fling.as_ref() else {
            return;
        };

        let now = std::time::Instant::now();
        let dt = now.duration_since(fling.last_time).as_secs_f32();
        let friction = 0.95_f32.powf(dt * 60.0);
        let new_vx = fling.velocity_x * friction;
        let new_vy = fling.velocity_y * friction;
        let position = fling.position;

        if new_vx.abs() < FLING_THRESHOLD && new_vy.abs() < FLING_THRESHOLD {
            self.touch_state.fling = None;
            self.handle_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Ended,
            }));
            return;
        }

        if let Some(fling) = self.touch_state.fling.as_mut() {
            fling.velocity_x = new_vx;
            fling.velocity_y = new_vy;
            fling.last_time = now;
        }

        self.handle_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(new_vx * dt), px(new_vy * dt))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }));
    }
}

pub type AndroidWindowStatePtr = Rc<RefCell<AndroidWindowState>>;

pub struct AndroidWindow {
    pub state: AndroidWindowStatePtr,
    // Window::draw temporarily takes the live handler every frame, and touch
    // dispatch may already hold state when keyboard methods are called.
    input_handler_registered: Cell<bool>,
    keyboard_session_requested: Cell<bool>,
}

impl AndroidWindow {
    pub fn new(
        _handle: AnyWindowHandle,
        role: AndroidWindowRole,
        window_handle: u64,
        bounds: Bounds<Pixels>,
        scale: f32,
        active: bool,
    ) -> Self {
        let state = AndroidWindowState::new(role, window_handle, bounds, scale, active);
        Self {
            state: Rc::new(RefCell::new(state)),
            input_handler_registered: Cell::new(false),
            keyboard_session_requested: Cell::new(false),
        }
    }

    pub fn handle_surface_created(
        &self,
        native_window: NativeWindow,
        gpu_context: GpuContext,
    ) -> Result<()> {
        self.state
            .borrow_mut()
            .handle_surface_created(native_window, gpu_context)
    }

    pub fn handle_surface_changed(&self, width: u32, height: u32) -> Result<()> {
        let resize_info = self
            .state
            .borrow_mut()
            .handle_surface_changed(width, height)?;
        if let Some((size, scale)) = resize_info {
            let mut callback = self.state.borrow_mut().take_resize_callback();
            if let Some(ref mut cb) = callback {
                cb(size, scale);
            }
            self.state.borrow_mut().restore_resize_callback(callback);
        }
        Ok(())
    }

    pub fn handle_surface_destroyed(&self) {
        self.state.borrow_mut().handle_surface_destroyed()
    }

    pub fn handle_input(&self, input: PlatformInput) -> DispatchEventResult {
        self.state.borrow_mut().handle_input(input)
    }

    pub fn request_frame(&self) {
        self.state.borrow_mut().request_frame()
    }
}

impl PlatformWindow for AndroidWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.state.borrow().bounds
    }

    fn is_maximized(&self) -> bool {
        true
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Maximized(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.state.borrow().bounds.size
    }

    fn resize(&mut self, _size: Size<Pixels>) {
        log::warn!("resize() called but Android windows cannot be manually resized");
    }

    fn scale_factor(&self) -> f32 {
        self.state.borrow().scale
    }

    fn appearance(&self) -> WindowAppearance {
        self.state.borrow().appearance
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.state.borrow().last_mouse_position
    }

    fn modifiers(&self) -> Modifiers {
        self.state.borrow().modifiers
    }

    fn capslock(&self) -> Capslock {
        Capslock { on: false }
    }

    fn set_selection_handler(&mut self, input_handler: PlatformInputHandler) {
        self.state.borrow_mut().selection_handler = Some(input_handler);
    }

    fn take_selection_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.borrow_mut().selection_handler.take()
    }

    fn clear_selection_handler(&mut self) {
        let dismissed_selection = {
            let mut state = self.state.borrow_mut();
            state.selection_handler = None;
            state.selectable_text_hit_regions.clear();
            // The read-only document is gone; drop any active selection sourced
            // from it so stale hit regions can't seed a new interaction.
            let dismissed = state.active_selection_source == Some(ActiveSelectionSource::ReadOnly);
            if dismissed {
                state.active_selection_source = None;
            }
            dismissed
        };
        if dismissed_selection {
            super::app_state::with_platform(|platform| platform.dismiss_selection());
        }
    }

    fn clear_active_selection(&self) {
        // GPUI cleared its own selection state; mirror that by dropping the
        // native presentation without re-clearing the handler.
        //
        // GPUI calls this reentrantly: our selection ops (e.g. `update_active_selection`,
        // `clear_active_selection`) run a handler closure while the platform holds
        // the window borrow, and GPUI can clear its selection from inside that
        // closure and call back here. The outer op already owns a consistent
        // resulting state, so a failed borrow must be a no-op rather than a panic.
        // The next per-frame refresh reflects the true state, so nothing is lost.
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        state.clear_active_selection(false);
        drop(state);
        super::app_state::with_platform(|platform| platform.dismiss_selection());
    }

    fn set_selectable_text_hit_regions(&self, regions: SmallVec<[SelectableTextHitRegion; 8]>) {
        self.state.borrow_mut().selectable_text_hit_regions = regions;
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        let mut input_handler = input_handler;
        let should_auto_request_keyboard = {
            let mut state = self.state.borrow_mut();
            let accepts_text_input = input_handler.query_accepts_text_input();
            let uses_manual_focus = input_handler.query_uses_manual_focus();
            state.input_keyboard_accessory =
                accepts_text_input && input_handler.query_keyboard_accessory();
            state.input_handler = Some(input_handler);
            let should_auto_request_keyboard = should_auto_request_soft_keyboard(
                accepts_text_input,
                uses_manual_focus,
                self.input_handler_registered.get(),
            );
            self.input_handler_registered.set(true);
            self.keyboard_session_requested
                .set(self.keyboard_session_requested.get() || should_auto_request_keyboard);
            should_auto_request_keyboard
        };

        if should_auto_request_keyboard {
            super::app_state::with_platform(|platform| platform.request_soft_keyboard());
        }
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.borrow_mut().input_handler.take()
    }

    fn clear_input_handler(&mut self) {
        let (had_keyboard_session, dismissed_selection) = {
            let mut state = self.state.borrow_mut();
            state.input_keyboard_accessory = false;
            let dismissed_selection =
                state.active_selection_source == Some(ActiveSelectionSource::Input);
            if dismissed_selection {
                state.active_selection_source = None;
            }
            state.input_handler.take();
            self.input_handler_registered.set(false);
            (
                self.keyboard_session_requested.replace(false),
                dismissed_selection,
            )
        };
        if had_keyboard_session {
            super::app_state::with_platform(|platform| platform.hide_soft_keyboard());
        }
        if dismissed_selection {
            super::app_state::with_platform(|platform| platform.dismiss_selection());
        }
    }

    fn show_soft_keyboard(&self) {
        self.keyboard_session_requested.set(true);
        super::app_state::with_platform(|platform| platform.request_soft_keyboard());
    }

    fn hide_soft_keyboard(&self) {
        self.keyboard_session_requested.set(false);
        super::app_state::with_platform(|platform| platform.hide_soft_keyboard());
    }

    fn is_soft_keyboard_visible(&self) -> bool {
        super::ffi::keyboard_height() > 0
    }

    fn has_active_keyboard_accessory(&self) -> bool {
        self.state.borrow_mut().has_active_keyboard_accessory()
    }

    fn completed_frame(&self) {
        if self.state.borrow().active_selection_source.is_some() {
            super::app_state::with_platform(|platform| platform.refresh_selection());
        }
    }
    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        None
    }

    fn activate(&self) {}

    fn is_active(&self) -> bool {
        self.state.borrow().active
    }

    fn is_hovered(&self) -> bool {
        false
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.state.borrow().background_appearance
    }

    fn set_title(&mut self, _title: &str) {}

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.state.borrow_mut().background_appearance = background_appearance;
    }

    fn minimize(&self) {}

    fn zoom(&self) {}

    fn toggle_fullscreen(&self) {}

    fn is_fullscreen(&self) -> bool {
        true
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.state.borrow_mut().callbacks.request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.state.borrow_mut().callbacks.input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.state.borrow_mut().callbacks.active_status_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.state.borrow_mut().callbacks.hover_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.state.borrow_mut().callbacks.resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.state.borrow_mut().callbacks.moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.state.borrow_mut().callbacks.should_close = Some(callback);
    }

    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.state.borrow_mut().callbacks.hit_test_window_control = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.state.borrow_mut().callbacks.close = Some(callback);
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.state.borrow_mut().callbacks.appearance_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        self.state.borrow_mut().draw(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.state.borrow().atlas.clone()
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        // Adapter capability is constant for the lifetime of the renderer; cache
        // it at renderer creation so paint_glyph (called per glyph) doesn't
        // acquire the renderer mutex on every call.
        self.state.borrow().subpixel_supported.unwrap_or(false)
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.state
            .borrow()
            .renderer
            .as_ref()
            .map(|renderer| renderer.lock().gpu_specs())
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
}

impl HasWindowHandle for AndroidWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

impl HasDisplayHandle for AndroidWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        let handle = AndroidDisplayHandle::new();
        Ok(unsafe {
            raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Android(handle))
        })
    }
}
