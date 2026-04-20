use std::{
    cell::RefCell,
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use futures::channel::oneshot;
use jni::{JavaVM, objects::GlobalRef};
use ndk::native_window::NativeWindow;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, HasDisplayHandle, HasWindowHandle,
    RawDisplayHandle, RawWindowHandle,
};

use gpui::{
    AnyWindowHandle, Bounds, Capslock, DevicePixels, DispatchEventResult, GpuSpecs, Modifiers,
    Pixels, PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow,
    Point, PromptButton, PromptLevel, RequestFrameOptions, Scene, Size, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea,
};
use gpui_wgpu::{WgpuAtlas, WgpuContext, WgpuRenderer, WgpuSurfaceConfig};

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
    raw_window: Option<RawWindow>,
    native_window: Option<NativeWindow>,
    renderer: Option<WgpuRenderer>,
    atlas: Arc<WgpuAtlas>,
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
        context: &WgpuContext,
    ) -> Result<Self> {
        let atlas = Arc::new(WgpuAtlas::new(
            Arc::clone(&context.device),
            Arc::clone(&context.queue),
        ));

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

    pub fn handle_surface_created(
        &mut self,
        native_window: NativeWindow,
        context: &WgpuContext,
    ) -> Result<()> {
        log::info!("AndroidWindow: surface created");

        let window_ptr = native_window.ptr().as_ptr() as *mut c_void;
        let raw_window = RawWindow { window: window_ptr };

        let size = Size {
            width: DevicePixels((f32::from(self.bounds.size.width) * self.scale) as i32),
            height: DevicePixels((f32::from(self.bounds.size.height) * self.scale) as i32),
        };

        log::info!(
            "Creating WgpuRenderer with physical size: {}x{} (logical: {}x{}, scale: {})",
            size.width.0,
            size.height.0,
            f32::from(self.bounds.size.width),
            f32::from(self.bounds.size.height),
            self.scale
        );

        let config = WgpuSurfaceConfig {
            size,
            transparent: matches!(
                self.background_appearance,
                WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred
            ),
        };

        let renderer =
            WgpuRenderer::new_with_atlas(context, &raw_window, config, Some(self.atlas.clone()))?;

        self.native_window = Some(native_window);
        self.raw_window = Some(raw_window);
        self.renderer = Some(renderer);

        Ok(())
    }

    pub fn handle_surface_changed(
        &mut self,
        width: u32,
        height: u32,
        context: &WgpuContext,
    ) -> Result<Option<(Size<Pixels>, f32)>> {
        log::info!("AndroidWindow: surface changed to {}x{}", width, height);

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

        if bounds_changed {
            if let Some(ref raw_window) = self.raw_window {
                let size = Size {
                    width: DevicePixels(width as i32),
                    height: DevicePixels(height as i32),
                };

                let config = WgpuSurfaceConfig {
                    size,
                    transparent: matches!(
                        self.background_appearance,
                        WindowBackgroundAppearance::Transparent
                            | WindowBackgroundAppearance::Blurred
                    ),
                };

                let renderer = WgpuRenderer::new_with_atlas(
                    context,
                    raw_window,
                    config,
                    Some(self.atlas.clone()),
                )?;
                self.renderer = Some(renderer);
            }
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
        log::info!("AndroidWindow: surface destroyed");

        self.renderer = None;
        self.native_window = None;
        self.raw_window = None;
    }

    pub fn draw(&mut self, scene: &Scene) {
        if let Some(ref mut renderer) = self.renderer {
            renderer.draw(scene);
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
}

pub type AndroidWindowStatePtr = Rc<RefCell<AndroidWindowState>>;

pub struct AndroidWindow {
    pub state: AndroidWindowStatePtr,
}

impl AndroidWindow {
    pub fn new(
        handle: AnyWindowHandle,
        bounds: Bounds<Pixels>,
        scale: f32,
        jvm: Arc<JavaVM>,
        activity: Arc<Mutex<GlobalRef>>,
        context: &WgpuContext,
    ) -> Result<Self> {
        let state = AndroidWindowState::new(handle, bounds, scale, jvm, activity, context)?;
        Ok(Self {
            state: Rc::new(RefCell::new(state)),
        })
    }

    pub fn handle_surface_created(
        &self,
        native_window: NativeWindow,
        context: &WgpuContext,
    ) -> Result<()> {
        self.state
            .borrow_mut()
            .handle_surface_created(native_window, context)
    }

    pub fn handle_surface_changed(
        &self,
        width: u32,
        height: u32,
        context: &WgpuContext,
    ) -> Result<()> {
        let resize_info = self
            .state
            .borrow_mut()
            .handle_surface_changed(width, height, context)?;
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
        false
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
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
