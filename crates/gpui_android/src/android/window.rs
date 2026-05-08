use std::{
    cell::RefCell,
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

use gpui::{
    AnyWindowHandle, Bounds, Capslock, DevicePixels, DispatchEventResult, GpuSpecs, Modifiers,
    Pixels, PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow,
    Point, PointerButton, PointerCancelEvent, PointerDownEvent, PointerKind, PointerMoveEvent,
    PointerUpEvent, PromptButton, PromptLevel, RequestFrameOptions, Scene, ScrollDelta,
    ScrollWheelEvent, Size, TouchPhase, WindowAppearance, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, point, px,
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
}

impl TouchState {
    fn new() -> Self {
        Self {
            last_position: None,
            down_position: None,
            is_drag: false,
            suppress_scroll: false,
            fling: None,
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

pub struct AndroidWindowState {
    role: AndroidWindowRole,
    raw_window: Option<RawWindow>,
    native_window: Option<NativeWindow>,
    renderer: Option<WgpuRenderer>,
    atlas: Arc<AndroidAtlas>,
    bounds: Bounds<Pixels>,
    scale: f32,
    touch_state: TouchState,
    input_handler: Option<PlatformInputHandler>,
    callbacks: Callbacks,
    active: bool,
    appearance: WindowAppearance,
    background_appearance: WindowBackgroundAppearance,
    modifiers: Modifiers,
    last_mouse_position: Point<Pixels>,
}

impl AndroidWindowState {
    pub fn new(role: AndroidWindowRole, bounds: Bounds<Pixels>, scale: f32, active: bool) -> Self {
        Self {
            role,
            raw_window: None,
            native_window: None,
            renderer: None,
            atlas: Arc::new(AndroidAtlas::new()),
            bounds,
            scale,
            touch_state: TouchState::new(),
            input_handler: None,
            callbacks: Callbacks::default(),
            active,
            appearance: WindowAppearance::Light,
            background_appearance: WindowBackgroundAppearance::Transparent,
            modifiers: Modifiers::default(),
            last_mouse_position: Point::default(),
        }
    }

    pub fn role(&self) -> AndroidWindowRole {
        self.role
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
            renderer.replace_surface(&raw_window, config, &context.instance)?;
        } else {
            let renderer = if let Some(atlas) = self.atlas.gpu_atlas() {
                WgpuRenderer::new_with_atlas(gpu_context, &raw_window, config, None, atlas)?
            } else {
                WgpuRenderer::new(gpu_context, &raw_window, config, None)?
            };
            self.atlas.bind(renderer.atlas());
            self.renderer = Some(renderer);
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
            renderer.update_drawable_size(Size {
                width: DevicePixels(width as i32),
                height: DevicePixels(height as i32),
            });
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
            renderer.unconfigure_surface();
        }
        self.native_window = None;
        self.raw_window = None;
    }

    pub fn draw(&mut self, scene: &Scene) {
        if let Some(ref mut renderer) = self.renderer {
            if renderer.device_lost() {
                if let Some(raw_window) = self.raw_window {
                    if let Err(error) = renderer.recover(&raw_window) {
                        log::error!("Failed to recover Android renderer: {error:?}");
                        return;
                    }
                } else {
                    return;
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
            preferred_present_mode: Some(gpui_wgpu::wgpu::PresentMode::Mailbox),
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
                self.touch_state.last_position = None;
                self.touch_state.down_position = None;
                self.touch_state.is_drag = false;

                if !is_drag || suppress_scroll {
                    // Java forwards velocity unconditionally. If the pointer
                    // handler prevented default, the matching fling must die
                    // with the suppressed synthetic scroll stream.
                    self.touch_state.fling = None;
                } else {
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
}

impl AndroidWindow {
    pub fn new(
        _handle: AnyWindowHandle,
        role: AndroidWindowRole,
        bounds: Bounds<Pixels>,
        scale: f32,
        active: bool,
    ) -> Self {
        let state = AndroidWindowState::new(role, bounds, scale, active);
        Self {
            state: Rc::new(RefCell::new(state)),
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

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.state.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.borrow_mut().input_handler.take()
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
        // Mirrors `WgpuRenderer::supports_dual_source_blending` — only true on
        // Vulkan adapters that expose the feature (most desktop GPUs and some
        // newer Adreno/PowerVR drivers). Mali-G68 and similar mobile GPUs
        // return false here and fall back to grayscale alpha rendering.
        self.state
            .borrow()
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.supports_dual_source_blending())
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.state
            .borrow()
            .renderer
            .as_ref()
            .map(|renderer| renderer.gpu_specs())
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
