use std::{
    cell::RefCell,
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{Arc, Mutex},
};

use blade_graphics as gpu;

use anyhow::{anyhow, Result};
use futures::channel::oneshot;
use jni::{
    objects::GlobalRef,
    JavaVM,
};
use ndk::native_window::NativeWindow;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, HasDisplayHandle, HasWindowHandle,
    RawDisplayHandle, RawWindowHandle,
};

use crate::{
    AnyWindowHandle, Bounds, Capslock, GpuSpecs, Modifiers, Pixels, PlatformAtlas,
    PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow, Point, PromptButton,
    PromptLevel, RequestFrameOptions, Size, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, scene::Scene,
};
use crate::platform::blade::{BladeAtlas, BladeContext, BladeRenderer, BladeSurfaceConfig};
use crate::DispatchEventResult;

/// Callbacks for window events
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

/// Raw window handle implementation for Android
struct RawWindow {
    window: *mut c_void,
}

unsafe impl Send for RawWindow {}
unsafe impl Sync for RawWindow {}

impl HasWindowHandle for RawWindow {
    fn window_handle(&self) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let window = NonNull::new(self.window).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = AndroidNdkWindowHandle::new(window.cast());
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(RawWindowHandle::AndroidNdk(handle)) })
    }
}

impl HasDisplayHandle for RawWindow {
    fn display_handle(&self) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        let handle = AndroidDisplayHandle::new();
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Android(handle)) })
    }
}

/// Internal state for an Android window
pub(crate) struct AndroidWindowState {
    raw_window: Option<RawWindow>,
    native_window: Option<NativeWindow>,
    renderer: Option<BladeRenderer>,
    atlas: Arc<BladeAtlas>,
    bounds: Bounds<Pixels>,
    scale: f32,
    input_handler: Option<PlatformInputHandler>,
    callbacks: Callbacks,
    active: bool,
    appearance: WindowAppearance,
    background_appearance: WindowBackgroundAppearance,
    jvm: Arc<JavaVM>,
    activity: Arc<Mutex<GlobalRef>>,
    handle: AnyWindowHandle,
    modifiers: Modifiers,
    last_mouse_position: Point<Pixels>,
}

impl AndroidWindowState {
    pub fn new(
        handle: AnyWindowHandle,
        bounds: Bounds<Pixels>,
        scale: f32,
        jvm: Arc<JavaVM>,
        activity: Arc<Mutex<GlobalRef>>,
        context: &BladeContext,
    ) -> Result<Self> {
        // Create the atlas for texture management
        let atlas = Arc::new(BladeAtlas::new(context.gpu_context()));

        Ok(Self {
            raw_window: None,
            native_window: None,
            renderer: None,
            atlas,
            bounds,
            scale,
            input_handler: None,
            callbacks: Callbacks::default(),
            active: false,
            appearance: WindowAppearance::Light,
            background_appearance: WindowBackgroundAppearance::Opaque,
            jvm,
            activity,
            handle,
            modifiers: Modifiers::default(),
            last_mouse_position: Point::default(),
        })
    }

    /// Handle surface created event from Android
    pub fn handle_surface_created(
        &mut self,
        native_window: NativeWindow,
        context: &BladeContext,
    ) -> Result<()> {
        log::info!("AndroidWindow: surface created");

        let window_ptr = native_window.ptr().as_ptr() as *mut c_void;
        let raw_window = RawWindow {
            window: window_ptr,
        };

        // Create the Blade renderer with PHYSICAL pixels (bounds * scale)
        let size = gpu::Extent {
            width: (self.bounds.size.width.0 * self.scale) as u32,
            height: (self.bounds.size.height.0 * self.scale) as u32,
            depth: 1,
        };

        log::info!("Creating BladeRenderer with physical size: {}x{} (logical: {}x{}, scale: {})",
            size.width, size.height,
            self.bounds.size.width.0, self.bounds.size.height.0, self.scale);

        let config = BladeSurfaceConfig {
            size,
            transparent: matches!(
                self.background_appearance,
                WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred
            ),
        };

        // CRITICAL: Use new_with_atlas to share the same atlas instance between the window
        // and renderer. The GPUI Window captures sprite_atlas() during construction BEFORE
        // the renderer exists, so the renderer must use the same atlas.
        let renderer = BladeRenderer::new_with_atlas(context, &raw_window, config, self.atlas.clone())?;

        self.native_window = Some(native_window);
        self.raw_window = Some(raw_window);
        self.renderer = Some(renderer);

        Ok(())
    }

    /// Handle surface changed event (resize/rotation)
    pub fn handle_surface_changed(&mut self, width: u32, height: u32, context: &BladeContext) -> Result<()> {
        log::info!("AndroidWindow: surface changed to {}x{}", width, height);

        let new_bounds = Bounds {
            origin: self.bounds.origin,
            size: Size {
                width: crate::px(width as f32 / self.scale),
                height: crate::px(height as f32 / self.scale),
            },
        };

        if new_bounds != self.bounds {
            self.bounds = new_bounds;

            // Recreate the renderer with new size
            if let Some(ref raw_window) = self.raw_window {
                let size = gpu::Extent {
                    width,
                    height,
                    depth: 1,
                };

                let config = BladeSurfaceConfig {
                    size,
                    transparent: matches!(
                        self.background_appearance,
                        WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred
                    ),
                };

                // Use the same shared atlas when recreating the renderer
                let renderer = BladeRenderer::new_with_atlas(context, raw_window, config, self.atlas.clone())?;
                self.renderer = Some(renderer);
            }

            // Notify resize callback
            if let Some(ref mut callback) = self.callbacks.resize {
                callback(new_bounds.size, self.scale);
            }
        }

        Ok(())
    }

    /// Handle surface destroyed event
    pub fn handle_surface_destroyed(&mut self) {
        log::info!("AndroidWindow: surface destroyed");

        // Drop the renderer first
        self.renderer = None;

        // Then drop the native window and raw window
        self.native_window = None;
        self.raw_window = None;
    }

    /// Draw a scene to the window
    pub fn draw(&mut self, scene: &Scene) {
        if let Some(ref mut renderer) = self.renderer {
            renderer.draw(scene);
        }
    }

    /// Request a frame to be rendered
    /// This should be called by the platform's event loop to trigger GPUI rendering
    pub fn request_frame(&mut self) {
        if let Some(ref mut callback) = self.callbacks.request_frame {
            callback(RequestFrameOptions::default());
        }
    }

    /// Extract the request_frame callback without calling it
    /// Returns the callback if it exists
    pub(crate) fn take_request_frame_callback(&mut self) -> Option<Box<dyn FnMut(RequestFrameOptions)>> {
        self.callbacks.request_frame.take()
    }

    /// Put back the request_frame callback
    pub(crate) fn put_request_frame_callback(&mut self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.callbacks.request_frame = Some(callback);
    }

    /// Handle input events
    pub fn handle_input(&mut self, input: PlatformInput) -> DispatchEventResult {
        if let Some(ref mut callback) = self.callbacks.input {
            callback(input)
        } else {
            DispatchEventResult::default()
        }
    }
}

pub(crate) type AndroidWindowStatePtr = Rc<RefCell<AndroidWindowState>>;

/// Public wrapper around AndroidWindowState
pub(crate) struct AndroidWindow {
    pub(crate) state: AndroidWindowStatePtr,
}

impl AndroidWindow {
    pub fn new(
        handle: AnyWindowHandle,
        bounds: Bounds<Pixels>,
        scale: f32,
        jvm: Arc<JavaVM>,
        activity: Arc<Mutex<GlobalRef>>,
        context: &BladeContext,
    ) -> Result<Self> {
        let state = AndroidWindowState::new(handle, bounds, scale, jvm, activity, context)?;
        Ok(Self {
            state: Rc::new(RefCell::new(state)),
        })
    }

    pub fn handle_surface_created(
        &self,
        native_window: NativeWindow,
        context: &BladeContext,
    ) -> Result<()> {
        self.state.borrow_mut().handle_surface_created(native_window, context)
    }

    pub fn handle_surface_changed(
        &self,
        width: u32,
        height: u32,
        context: &BladeContext,
    ) -> Result<()> {
        self.state.borrow_mut().handle_surface_changed(width, height, context)
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
        // Android apps are typically always "maximized" (fullscreen)
        true
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Maximized(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.state.borrow().bounds.size
    }

    fn resize(&mut self, _size: Size<Pixels>) {
        // On Android, window size is controlled by the system
        log::warn!("resize() called but Android windows cannot be manually resized");
    }

    fn scale_factor(&self) -> f32 {
        self.state.borrow().scale
    }

    fn appearance(&self) -> WindowAppearance {
        self.state.borrow().appearance
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        // TODO: Return the actual display
        None
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.state.borrow().last_mouse_position
    }

    fn modifiers(&self) -> Modifiers {
        self.state.borrow().modifiers
    }

    fn capslock(&self) -> Capslock {
        // TODO: Track capslock state
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
        // TODO: Implement native Android dialogs via JNI
        None
    }

    fn activate(&self) {
        // Android handles window activation
    }

    fn is_active(&self) -> bool {
        self.state.borrow().active
    }

    fn is_hovered(&self) -> bool {
        // Android doesn't have a hover concept (touch-based)
        false
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.state.borrow().background_appearance
    }

    fn set_title(&mut self, _title: &str) {
        // Android apps typically don't have window titles
    }

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.state.borrow_mut().background_appearance = background_appearance;
    }

    fn minimize(&self) {
        // On Android, minimize means moving to background
        // TODO: Implement via JNI to move task to back
    }

    fn zoom(&self) {
        // Not applicable on Android
    }

    fn toggle_fullscreen(&self) {
        // Android apps are typically always fullscreen
    }

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
        // Android typically uses grayscale rendering for text
        false
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        // TODO: Query actual GPU specs from the device
        None
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {
        // TODO: Update soft keyboard position via JNI
    }
}

impl HasWindowHandle for AndroidWindow {
    fn window_handle(&self) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // We can't safely return a handle that borrows from RefCell
        // For now, return Unavailable - this will be fixed when GPUI is fully integrated
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

impl HasDisplayHandle for AndroidWindow {
    fn display_handle(&self) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        // Android display handle doesn't borrow anything
        let handle = AndroidDisplayHandle::new();
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Android(handle)) })
    }
}
