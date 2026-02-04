use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::{Rc, Weak as RcWeak},
    sync::{Arc, Mutex, Weak as SyncWeak},
};

use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use jni::{JavaVM, objects::GlobalRef};
use ndk::looper::ThreadLooper;
use util::ResultExt;

use crate::platform::blade::BladeContext;
use crate::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, DisplayId,
    DispatchEventResult, ForegroundExecutor, Keymap, Menu, MenuItem, OwnedMenu, PathPromptOptions,
    Platform, PlatformDisplay, PlatformInput, PlatformKeyboardLayout, PlatformKeyboardMapper,
    PlatformTextSystem, PlatformWindow, RequestFrameOptions, RunnableVariant, Task,
    WindowAppearance, WindowParams, px,
};

use super::{AndroidDispatcher, AndroidKeyboardLayout, AndroidQueueReceiver, AndroidWindow, AndroidWindowState, CosmicTextSystem};

pub(crate) const DOUBLE_CLICK_DISTANCE: crate::Pixels = px(5.0);

/// Trait that defines the interface for AndroidPlatform to access shared state.
/// This mirrors the LinuxClient pattern.
pub trait AndroidClient {
    fn with_common<R>(&self, f: impl FnOnce(&mut AndroidCommon) -> R) -> R;
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>>;
    fn display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>>;
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>>;
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>>;
    fn set_cursor_style(&self, style: CursorStyle);
    fn open_uri(&self, uri: &str);
    fn write_to_clipboard(&self, item: ClipboardItem);
    fn read_from_clipboard(&self) -> Option<ClipboardItem>;
    fn run(&self);
}

/// Platform-specific handlers for lifecycle events.
#[derive(Default)]
pub(crate) struct PlatformHandlers {
    pub(crate) open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
    pub(crate) quit: Option<Box<dyn FnMut()>>,
    pub(crate) reopen: Option<Box<dyn FnMut()>>,
    pub(crate) app_menu_action: Option<Box<dyn FnMut(&dyn Action)>>,
    pub(crate) will_open_app_menu: Option<Box<dyn FnMut()>>,
    pub(crate) validate_app_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    pub(crate) keyboard_layout_change: Option<Box<dyn FnMut()>>,
}

/// Shared state for the Android platform.
/// This is analogous to LinuxCommon.
pub(crate) struct AndroidCommon {
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) text_system: Arc<dyn PlatformTextSystem>,
    pub(crate) appearance: WindowAppearance,
    pub(crate) callbacks: PlatformHandlers,
    pub(crate) quit_requested: bool,
    pub(crate) menus: Vec<OwnedMenu>,
    pub(crate) jvm: Arc<JavaVM>,
    pub(crate) activity: Arc<Mutex<GlobalRef>>,
}

impl AndroidCommon {
    pub fn new(
        liveness: SyncWeak<()>,
        jvm: Arc<JavaVM>,
        activity: Arc<Mutex<GlobalRef>>,
    ) -> (Self, AndroidQueueReceiver<RunnableVariant>) {
        let (main_sender, main_receiver) = AndroidQueueReceiver::new();
        let text_system = Arc::new(CosmicTextSystem::new());
        let dispatcher = Arc::new(AndroidDispatcher::new(main_sender));

        let common = AndroidCommon {
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher, liveness),
            text_system,
            appearance: WindowAppearance::Light,
            callbacks: PlatformHandlers::default(),
            quit_requested: false,
            menus: Vec::new(),
            jvm,
            activity,
        };

        (common, main_receiver)
    }
}

/// The main Android platform implementation.
pub struct AndroidPlatform {
    common: RefCell<AndroidCommon>,
    main_receiver: RefCell<AndroidQueueReceiver<RunnableVariant>>,
    jvm: Arc<JavaVM>,
    activity: Arc<Mutex<GlobalRef>>,
    blade_context: RefCell<Option<Arc<BladeContext>>>,
    windows: RefCell<Vec<RcWeak<RefCell<AndroidWindowState>>>>,
    display_scale: Cell<f32>,
}

impl AndroidPlatform {
    /// Creates a new AndroidPlatform instance.
    pub fn new(liveness: SyncWeak<()>, jvm: JavaVM, activity: GlobalRef) -> Self {
        let jvm = Arc::new(jvm);
        let activity = Arc::new(Mutex::new(activity));
        let (common, main_receiver) = AndroidCommon::new(liveness, jvm.clone(), activity.clone());

        // Load essential fonts synchronously for fast startup
        Self::load_essential_fonts(&common.text_system);

        // Load remaining system fonts in background
        let text_system_bg = common.text_system.clone();
        std::thread::spawn(move || Self::load_system_fonts(&text_system_bg));

        Self {
            common: RefCell::new(common),
            main_receiver: RefCell::new(main_receiver),
            jvm,
            activity,
            blade_context: RefCell::new(None),
            windows: RefCell::new(Vec::new()),
            display_scale: Cell::new(3.0),
        }
    }

    /// Set the display scale factor (from DisplayMetrics density).
    /// Must be called before opening any windows.
    pub fn set_display_scale(&self, scale: f32) {
        self.display_scale.set(scale);
    }

    /// Attach a native window to the most recently created AndroidWindow
    pub fn attach_native_window(&self, native_window: ndk::native_window::NativeWindow) -> Result<()> {
        let windows = self.windows.borrow();
        let window = windows
            .last()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| anyhow!("No windows available to attach surface"))?;

        let blade_context = self.ensure_blade_context()?;
        window.borrow_mut().handle_surface_created(native_window, &blade_context)
    }

    /// Detach native window from the most recently created AndroidWindow
    pub fn detach_native_window(&self) {
        if let Some(window) = self.windows.borrow().last().and_then(|w| w.upgrade()) {
            window.borrow_mut().handle_surface_destroyed();
        }
    }

    /// Handle surface size change for the most recently created window
    pub fn handle_surface_resize(&self, width: u32, height: u32) -> Result<()> {
        let windows = self.windows.borrow();
        let window = windows
            .last()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| anyhow!("No windows available for surface resize"))?;

        let blade_context = self.ensure_blade_context()?;

        // handle_surface_changed returns pending resize info so the callback
        // can be fired after the mutable borrow is released.
        let resize_info = window.borrow_mut().handle_surface_changed(width, height, &blade_context)?;
        if let Some((size, scale)) = resize_info {
            let mut callback = window.borrow_mut().take_resize_callback();
            if let Some(ref mut cb) = callback {
                cb(size, scale);
            }
            window.borrow_mut().restore_resize_callback(callback);
        }
        Ok(())
    }

    /// Request a frame to be rendered on all windows
    pub fn request_frame_for_all_windows(&self) {
        // Collect callbacks while borrowing windows
        let callbacks: Vec<_> = {
            let windows = self.windows.borrow();
            windows
                .iter()
                .filter_map(|w| w.upgrade())
                .filter_map(|window| {
                    window.borrow_mut().take_request_frame_callback().map(|cb| (window.clone(), cb))
                })
                .collect()
        };

        // Call callbacks and return them
        for (window, mut callback) in callbacks {
            callback(RequestFrameOptions::default());
            window.borrow_mut().put_request_frame_callback(callback);
        }

        // Clean up dead window references
        self.windows.borrow_mut().retain(|w| w.strong_count() > 0);
    }

    /// Dispatch a platform input event to the most recently created window
    pub fn dispatch_input(&self, input: PlatformInput) -> DispatchEventResult {
        let windows = self.windows.borrow();
        if let Some(window) = windows.last().and_then(|w| w.upgrade()) {
            window.borrow_mut().handle_input(input)
        } else {
            DispatchEventResult::default()
        }
    }

    /// Get or create the BladeContext
    fn ensure_blade_context(&self) -> Result<Arc<BladeContext>> {
        let mut blade_context = self.blade_context.borrow_mut();
        if let Some(ref ctx) = *blade_context {
            return Ok(ctx.clone());
        }

        let ctx = BladeContext::new()
            .or_else(|_| {
                // Retry with lenient settings
                unsafe { std::env::set_var("BLADE_PERMISSIVE", "1") };
                BladeContext::new()
            })
            .map_err(|e| anyhow!("Failed to create BladeContext: {:?}", e))?;

        let ctx = Arc::new(ctx);
        *blade_context = Some(ctx.clone());
        Ok(ctx)
    }

    /// Load essential fonts synchronously for fast startup
    fn load_essential_fonts(text_system: &Arc<dyn PlatformTextSystem>) {
        const ESSENTIAL_FONTS: &[&str] = &[
            "/system/fonts/Roboto-Regular.ttf",
            "/system/fonts/DroidSans.ttf",
            "/system/fonts/DroidSans-Bold.ttf",
            "/system/fonts/DroidSansMono.ttf",
            "/system/fonts/MiSansC_3.005.ttf",
            "/system/fonts/NotoColorEmoji.ttf",
        ];

        for path in ESSENTIAL_FONTS {
            if let Ok(data) = std::fs::read(path) {
                let _ = text_system.add_fonts(vec![std::borrow::Cow::Owned(data)]);
            }
        }
    }

    /// Load all system fonts (called in background thread)
    fn load_system_fonts(text_system: &Arc<dyn PlatformTextSystem>) {
        const FONT_DIRS: &[&str] = &["/system/fonts", "/vendor/fonts"];
        const FONT_EXTENSIONS: &[&str] = &["ttf", "otf", "ttc"];

        for dir in FONT_DIRS {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_font = path.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| FONT_EXTENSIONS.contains(&ext));

                if is_font {
                    if let Ok(data) = std::fs::read(&path) {
                        let _ = text_system.add_fonts(vec![std::borrow::Cow::Owned(data)]);
                    }
                }
            }
        }
    }
}

impl AndroidClient for AndroidPlatform {
    fn with_common<R>(&self, f: impl FnOnce(&mut AndroidCommon) -> R) -> R {
        f(&mut self.common.borrow_mut())
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        // TODO: Implement display enumeration via JNI
        // For now, return a default display
        vec![]
    }

    fn display(&self, _id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        // TODO: Implement display lookup via JNI
        None
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        // TODO: Implement primary display via JNI
        None
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let blade_context = self.ensure_blade_context()?;
        let scale = self.display_scale.get();

        let window = AndroidWindow::new(
            handle,
            options.bounds,
            scale,
            self.jvm.clone(),
            self.activity.clone(),
            &blade_context,
        )?;

        self.windows.borrow_mut().push(Rc::downgrade(&window.state));
        Ok(Box::new(window))
    }

    fn set_cursor_style(&self, _style: CursorStyle) {
        // Android doesn't have a traditional cursor
        // This is a no-op
    }

    fn open_uri(&self, uri: &str) {
        let uri = uri.to_string();
        let jvm = self.jvm.clone();
        let activity = self.activity.clone();

        self.with_common(|common| {
            common
                .background_executor
                .spawn(async move {
                    // TODO: Use JNI to launch an Intent with ACTION_VIEW
                    let _ = (jvm, activity, uri);
                })
                .detach();
        });
    }

    fn write_to_clipboard(&self, _item: ClipboardItem) {
        // TODO: Implement clipboard via JNI using ClipboardManager
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        // TODO: Implement clipboard reading via JNI using ClipboardManager
        None
    }

    fn run(&self) {
        // Android's event loop is driven by JNI callbacks, not a traditional run loop
        // The system calls our JNI methods which then process pending tasks
        // This method is called after on_finish_launching completes

        // On Android, we use the thread's looper to handle events
        if let Some(looper) = ThreadLooper::for_thread() {
            // Process events until quit is requested
            loop {
                // Check if quit was requested
                if self.with_common(|common| common.quit_requested) {
                    break;
                }

                // Poll the looper with a timeout
                // This will wait for events (like frame callbacks, input, etc.)
                match looper.poll_once_timeout(std::time::Duration::from_millis(100)) {
                    Ok(_) => {
                        // Process any pending runnables
                        self.process_pending_tasks();
                    }
                    Err(_) => {
                        // Error polling, exit loop
                        break;
                    }
                }
            }
        } else {
            // No looper available, just process tasks until quit
            loop {
                if self.with_common(|common| common.quit_requested) {
                    break;
                }

                self.process_pending_tasks();
                std::thread::sleep(std::time::Duration::from_millis(16)); // ~60 FPS
            }
        }
    }
}

impl AndroidPlatform {
    /// Process pending tasks from the main receiver
    fn process_pending_tasks(&self) {
        let mut main_receiver = self.main_receiver.borrow_mut();
        while let Some(runnable) = main_receiver.try_recv() {
            match runnable {
                RunnableVariant::Meta(runnable) => {
                    if runnable.metadata().is_app_alive() {
                        runnable.run();
                    }
                }
                RunnableVariant::Compat(runnable) => {
                    runnable.run();
                }
            }
        }
    }
}

// Implement the Platform trait by delegating to AndroidClient methods
impl<P: AndroidClient + 'static> Platform for P {
    fn background_executor(&self) -> BackgroundExecutor {
        self.with_common(|common| common.background_executor.clone())
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.with_common(|common| common.foreground_executor.clone())
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.with_common(|common| common.text_system.clone())
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        // TODO: Implement Android keyboard layout detection via JNI
        Box::new(AndroidKeyboardLayout::new("default".into()))
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(crate::DummyKeyboardMapper)
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.with_common(|common| common.callbacks.keyboard_layout_change = Some(callback));
    }

    fn run(&self, on_finish_launching: Box<dyn FnOnce()>) {
        on_finish_launching();

        AndroidClient::run(self);

        let quit = self.with_common(|common| common.callbacks.quit.take());
        if let Some(mut fun) = quit {
            fun();
        }
    }

    fn quit(&self) {
        self.with_common(|common| common.quit_requested = true);
    }

    fn compositor_name(&self) -> &'static str {
        "Android"
    }

    fn restart(&self, _binary_path: Option<PathBuf>) {
        log::warn!("restart() is not implemented on Android");
    }

    fn activate(&self, _ignoring_other_apps: bool) {
        // On Android, apps are typically brought to foreground by the system
    }

    fn hide(&self) {
        // On Android, apps cannot programmatically hide themselves
    }

    fn hide_other_apps(&self) {
        // Not applicable on Android
    }

    fn unhide_other_apps(&self) {
        // Not applicable on Android
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        AndroidClient::primary_display(self)
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        AndroidClient::displays(self)
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        // TODO: Track active window
        None
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        AndroidClient::open_window(self, handle, options)
    }

    fn open_url(&self, url: &str) {
        self.open_uri(url);
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.with_common(|common| common.callbacks.open_urls = Some(callback));
    }

    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (done_tx, done_rx) = oneshot::channel();
        // TODO: Implement file picker using Storage Access Framework
        done_tx.send(Ok(None)).ok();
        done_rx
    }

    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (done_tx, done_rx) = oneshot::channel();
        // TODO: Implement file save dialog
        done_tx.send(Ok(None)).ok();
        done_rx
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    fn reveal_path(&self, _path: &Path) {
        // TODO: Implement file revealing (e.g., open file manager)
    }

    fn open_with_system(&self, path: &Path) {
        let path = path.to_owned();
        self.background_executor()
            .spawn(async move {
                // TODO: Use Intent.ACTION_VIEW to open file
                let _ = path;
            })
            .detach();
    }

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.with_common(|common| {
            common.callbacks.quit = Some(callback);
        });
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.with_common(|common| {
            common.callbacks.reopen = Some(callback);
        });
    }

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.with_common(|common| {
            common.callbacks.app_menu_action = Some(callback);
        });
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.with_common(|common| {
            common.callbacks.will_open_app_menu = Some(callback);
        });
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.with_common(|common| {
            common.callbacks.validate_app_menu_command = Some(callback);
        });
    }

    fn app_path(&self) -> Result<PathBuf> {
        // On Android, return the app's data directory
        Ok(PathBuf::from("/data/data/dev.zedra.app"))
    }

    fn set_menus(&self, menus: Vec<Menu>, _keymap: &Keymap) {
        self.with_common(|common| {
            common.menus = menus.into_iter().map(|menu| menu.owned()).collect();
        })
    }

    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.with_common(|common| Some(common.menus.clone()))
    }

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {
        // Not applicable on Android
    }

    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow!(
            "path_for_auxiliary_executable is not implemented on Android"
        ))
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        self.set_cursor_style(style)
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        // On Android, scrollbars are typically auto-hidden
        true
    }

    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        // TODO: Implement secure credential storage using Android KeyStore
        Task::ready(Err(anyhow!("write_credentials not implemented")))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        // TODO: Implement credential reading from Android KeyStore
        Task::ready(Ok(None))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        // TODO: Implement credential deletion from Android KeyStore
        Task::ready(Ok(()))
    }

    fn window_appearance(&self) -> WindowAppearance {
        self.with_common(|common| common.appearance)
    }

    fn register_url_scheme(&self, _: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!("register_url_scheme unimplemented on Android")))
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        AndroidClient::write_to_clipboard(self, item)
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        AndroidClient::read_from_clipboard(self)
    }
}
