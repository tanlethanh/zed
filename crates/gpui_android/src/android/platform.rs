use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::{Rc, Weak as RcWeak},
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use jni::{JavaVM, objects::GlobalRef};
use ndk::looper::ThreadLooper;

use gpui_wgpu::WgpuContext;
use gpui::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle,
    DummyKeyboardMapper, DispatchEventResult, ForegroundExecutor, Keymap, Menu, MenuItem,
    OwnedMenu, PathPromptOptions, Platform, PlatformDisplay, PlatformInput,
    PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow,
    RequestFrameOptions, RunnableVariant, Task, ThermalState, WindowAppearance, WindowParams, px,
};

use super::dispatcher::{AndroidDispatcher, AndroidQueueReceiver};
use super::keyboard::AndroidKeyboardLayout;
use super::text_system::CosmicTextSystem;
use super::window::{AndroidWindow, AndroidWindowState};

pub const DOUBLE_CLICK_DISTANCE: gpui::Pixels = px(5.0);

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

pub struct AndroidCommon {
    pub background_executor: BackgroundExecutor,
    pub foreground_executor: ForegroundExecutor,
    pub text_system: Arc<dyn PlatformTextSystem>,
    pub appearance: WindowAppearance,
    pub(crate) callbacks: PlatformHandlers,
    pub quit_requested: bool,
    pub(crate) menus: Vec<OwnedMenu>,
    pub jvm: Arc<JavaVM>,
    pub activity: Arc<Mutex<GlobalRef>>,
}

impl AndroidCommon {
    pub fn new(
        jvm: Arc<JavaVM>,
        activity: Arc<Mutex<GlobalRef>>,
    ) -> (Self, AndroidQueueReceiver<RunnableVariant>) {
        let (main_sender, main_receiver) = AndroidQueueReceiver::new();
        let text_system = Arc::new(CosmicTextSystem::new());
        let dispatcher = Arc::new(AndroidDispatcher::new(main_sender));

        let common = AndroidCommon {
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher),
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

pub struct AndroidPlatform {
    common: RefCell<AndroidCommon>,
    main_receiver: RefCell<AndroidQueueReceiver<RunnableVariant>>,
    jvm: Arc<JavaVM>,
    activity: Arc<Mutex<GlobalRef>>,
    wgpu_context: RefCell<Option<Arc<WgpuContext>>>,
    windows: RefCell<Vec<RcWeak<RefCell<AndroidWindowState>>>>,
    display_scale: Cell<f32>,
}

impl AndroidPlatform {
    pub fn new(jvm: JavaVM, activity: GlobalRef) -> Self {
        let jvm = Arc::new(jvm);
        let activity = Arc::new(Mutex::new(activity));
        let (common, main_receiver) = AndroidCommon::new(jvm.clone(), activity.clone());

        Self::load_essential_fonts(&common.text_system);

        let text_system_bg = common.text_system.clone();
        std::thread::spawn(move || Self::load_system_fonts(&text_system_bg));

        Self {
            common: RefCell::new(common),
            main_receiver: RefCell::new(main_receiver),
            jvm,
            activity,
            wgpu_context: RefCell::new(None),
            windows: RefCell::new(Vec::new()),
            display_scale: Cell::new(3.0),
        }
    }

    pub fn set_display_scale(&self, scale: f32) {
        self.display_scale.set(scale);
    }

    pub fn attach_native_window(&self, native_window: ndk::native_window::NativeWindow) -> Result<()> {
        let windows = self.windows.borrow();
        let window = windows
            .last()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| anyhow!("No windows available to attach surface"))?;

        let wgpu_context = self.ensure_wgpu_context()?;
        window.borrow_mut().handle_surface_created(native_window, &wgpu_context)
    }

    pub fn detach_native_window(&self) {
        if let Some(window) = self.windows.borrow().last().and_then(|w| w.upgrade()) {
            window.borrow_mut().handle_surface_destroyed();
        }
    }

    pub fn handle_surface_resize(&self, width: u32, height: u32) -> Result<()> {
        let windows = self.windows.borrow();
        let window = windows
            .last()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| anyhow!("No windows available for surface resize"))?;

        let wgpu_context = self.ensure_wgpu_context()?;

        let resize_info = window.borrow_mut().handle_surface_changed(width, height, &wgpu_context)?;
        if let Some((size, scale)) = resize_info {
            let mut callback = window.borrow_mut().take_resize_callback();
            if let Some(ref mut cb) = callback {
                cb(size, scale);
            }
            window.borrow_mut().restore_resize_callback(callback);
        }
        Ok(())
    }

    pub fn request_frame_for_all_windows(&self) {
        self.request_frame_with_options(false);
    }

    pub fn request_frame_forced(&self) {
        self.request_frame_with_options(true);
    }

    fn request_frame_with_options(&self, force_render: bool) {
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

        for (window, mut callback) in callbacks {
            callback(RequestFrameOptions {
                require_presentation: false,
                force_render,
            });
            window.borrow_mut().put_request_frame_callback(callback);
        }

        self.windows.borrow_mut().retain(|w| w.strong_count() > 0);
    }

    pub fn dispatch_input(&self, input: PlatformInput) -> DispatchEventResult {
        let window = {
            let windows = self.windows.borrow();
            windows.last().and_then(|w| w.upgrade())
        };

        let Some(window) = window else {
            return DispatchEventResult::default();
        };

        let mut callback = window.borrow_mut().take_input_callback();
        let result = if let Some(ref mut cb) = callback {
            cb(input)
        } else {
            DispatchEventResult::default()
        };
        window.borrow_mut().restore_input_callback(callback);
        result
    }

    pub fn process_pending_tasks(&self) {
        let mut main_receiver = self.main_receiver.borrow_mut();
        while let Some(runnable) = main_receiver.try_recv() {
            if !runnable.metadata().is_closed() {
                runnable.run();
            }
        }
    }

    fn with_common<R>(&self, f: impl FnOnce(&mut AndroidCommon) -> R) -> R {
        f(&mut self.common.borrow_mut())
    }

    fn ensure_wgpu_context(&self) -> Result<Arc<WgpuContext>> {
        let mut wgpu_context = self.wgpu_context.borrow_mut();
        if let Some(ref ctx) = *wgpu_context {
            return Ok(ctx.clone());
        }

        let ctx = WgpuContext::new()
            .map_err(|e| anyhow!("Failed to create WgpuContext: {:?}", e))?;

        let ctx = Arc::new(ctx);
        *wgpu_context = Some(ctx.clone());
        Ok(ctx)
    }

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

impl Platform for AndroidPlatform {
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
        Box::new(AndroidKeyboardLayout::new("default".into()))
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.with_common(|common| common.callbacks.keyboard_layout_change = Some(callback));
    }

    fn run(&self, on_finish_launching: Box<dyn FnOnce()>) {
        on_finish_launching();

        if let Some(looper) = ThreadLooper::for_thread() {
            loop {
                if self.with_common(|common| common.quit_requested) {
                    break;
                }

                match looper.poll_once_timeout(std::time::Duration::from_millis(100)) {
                    Ok(_) => {
                        self.process_pending_tasks();
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        } else {
            loop {
                if self.with_common(|common| common.quit_requested) {
                    break;
                }

                self.process_pending_tasks();
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }

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

    fn activate(&self, _ignoring_other_apps: bool) {}

    fn hide(&self) {}

    fn hide_other_apps(&self) {}

    fn unhide_other_apps(&self) {}

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![]
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        None
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let wgpu_context = self.ensure_wgpu_context()?;
        let scale = self.display_scale.get();

        let window = AndroidWindow::new(
            handle,
            options.bounds,
            scale,
            self.jvm.clone(),
            self.activity.clone(),
            &wgpu_context,
        )?;

        self.windows.borrow_mut().push(Rc::downgrade(&window.state));
        Ok(Box::new(window))
    }

    fn open_url(&self, _url: &str) {}

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.with_common(|common| common.callbacks.open_urls = Some(callback));
    }

    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (done_tx, done_rx) = oneshot::channel();
        done_tx.send(Ok(None)).ok();
        done_rx
    }

    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (done_tx, done_rx) = oneshot::channel();
        done_tx.send(Ok(None)).ok();
        done_rx
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    fn reveal_path(&self, _path: &Path) {}

    fn open_with_system(&self, _path: &Path) {}

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

    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }

    fn on_thermal_state_change(&self, _callback: Box<dyn FnMut()>) {}

    fn app_path(&self) -> Result<PathBuf> {
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

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}

    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow!(
            "path_for_auxiliary_executable is not implemented on Android"
        ))
    }

    fn set_cursor_style(&self, _style: CursorStyle) {}

    fn should_auto_hide_scrollbars(&self) -> bool {
        true
    }

    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(anyhow!("write_credentials not implemented")))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn window_appearance(&self) -> WindowAppearance {
        self.with_common(|common| common.appearance)
    }

    fn register_url_scheme(&self, _: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!("register_url_scheme unimplemented on Android")))
    }

    fn write_to_clipboard(&self, _item: ClipboardItem) {}

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        None
    }
}
