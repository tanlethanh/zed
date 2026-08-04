//! FFI (Foreign Function Interface) module for iOS.
//!
//! This module exposes C-compatible functions that can be called from
//! Objective-C code in the iOS app delegate to initialize and control
//! the GPUI application lifecycle.

use gpui::*;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Mutex, OnceLock};

/// Global storage for the GPUI application state.
/// This is set during initialization and used by FFI callbacks.
static IOS_APP_STATE: OnceLock<IosAppState> = OnceLock::new();

/// Holds the state needed for iOS FFI callbacks.
/// Note: On iOS, all UI code runs on the main thread, so we use a RefCell
/// instead of Mutex and don't require Send.
struct IosAppState {
    /// The callback to invoke when the app finishes launching.
    /// This is the closure passed to Application::run().
    /// Using std::cell::UnsafeCell since this is only accessed from the main thread.
    finish_launching: std::cell::UnsafeCell<Option<Box<dyn FnOnce()>>>,
}

// Safety: On iOS, all GPUI operations happen on the main thread.
// The FFI functions are only called from the iOS app delegate which runs on main thread.
// We implement both Send and Sync because OnceLock requires Send for its value type,
// and we need Sync for the static. The actual access is always single-threaded.
unsafe impl Send for IosAppState {}
unsafe impl Sync for IosAppState {}

// Safety wrapper for window list - only accessed from main thread
struct WindowListWrapper(std::cell::UnsafeCell<Vec<*const super::window::IosWindow>>);
unsafe impl Send for WindowListWrapper {}
unsafe impl Sync for WindowListWrapper {}

static IOS_WINDOW_LIST: OnceLock<WindowListWrapper> = OnceLock::new();

#[derive(Clone, Copy)]
pub(crate) struct EmbeddedWindowConfig {
    pub parent_view: *mut objc::runtime::Object,
    pub width_pts: f32,
    pub height_pts: f32,
}

// Safety: UIKit pointers are only used from the main thread. This wrapper only
// exists so the config can sit behind a static Mutex.
unsafe impl Send for EmbeddedWindowConfig {}

static PENDING_EMBEDDED_WINDOW: OnceLock<Mutex<Option<EmbeddedWindowConfig>>> = OnceLock::new();

/// Initialize the GPUI iOS application.
///
/// This should be called from `application:didFinishLaunchingWithOptions:`
/// in the iOS app delegate, before any other GPUI functions.
///
/// Returns a pointer to the app state that should be passed to other FFI functions.
/// Returns null if initialization fails.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_initialize() -> *mut c_void {
    // Initialize logging - iOS logging is typically handled via os_log
    // or NSLog, but for debug builds we can try to use env_logger if available
    #[cfg(all(debug_assertions, feature = "test-support"))]
    {
        // Try to initialize logging, ignore if already initialized
        let _ = env_logger::try_init();
    }

    log::info!("GPUI iOS: Initializing");

    // Initialize the app state
    let state = IosAppState {
        finish_launching: std::cell::UnsafeCell::new(None),
    };

    if IOS_APP_STATE.set(state).is_err() {
        log::error!("GPUI iOS: Already initialized");
        return std::ptr::null_mut();
    }

    // Initialize the window list
    let _ = IOS_WINDOW_LIST.set(WindowListWrapper(std::cell::UnsafeCell::new(Vec::new())));
    let _ = PENDING_EMBEDDED_WINDOW.set(Mutex::new(None));

    // Return a non-null pointer to indicate success
    // The actual state is stored in the static
    1 as *mut c_void
}

/// Register a window with the FFI layer.
///
/// This is called internally when a new IosWindow is created.
/// The window pointer can then be retrieved by Objective-C code.
///
/// # Safety
/// This must only be called from the main thread.
pub(crate) fn register_window(window: *const super::window::IosWindow) {
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            (*wrapper.0.get()).push(window);
            log::info!("GPUI iOS: Registered window {:p}", window);
        }
    }
}

pub(crate) fn unregister_window(window: *const super::window::IosWindow) {
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            let windows = &mut *wrapper.0.get();
            windows.retain(|&entry| entry != window);
        }
    }
}

pub(crate) fn take_pending_embedded_window() -> Option<EmbeddedWindowConfig> {
    let pending = PENDING_EMBEDDED_WINDOW.get()?;
    pending.lock().ok()?.take()
}

#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_set_next_embedded_parent(
    parent_view_ptr: *mut c_void,
    width_pts: f32,
    height_pts: f32,
) {
    let Some(pending) = PENDING_EMBEDDED_WINDOW.get() else {
        return;
    };
    let Ok(mut slot) = pending.lock() else {
        return;
    };

    *slot = Some(EmbeddedWindowConfig {
        parent_view: parent_view_ptr as *mut objc::runtime::Object,
        width_pts,
        height_pts,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_attach_embedded_view(
    window_ptr: *mut c_void,
    parent_view_ptr: *mut c_void,
    width_pts: f32,
    height_pts: f32,
) {
    if window_ptr.is_null() || parent_view_ptr.is_null() {
        return;
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.attach_to_parent(
        parent_view_ptr as *mut objc::runtime::Object,
        width_pts,
        height_pts,
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_detach_embedded_view(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.detach_from_parent();
}

/// Get the most recently created window pointer.
///
/// Returns the pointer to the IosWindow that was most recently registered,
/// or null if no windows have been created.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_get_window() -> *mut c_void {
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            let windows = &*wrapper.0.get();
            if let Some(&window) = windows.last() {
                return window as *mut c_void;
            }
        }
    }
    log::warn!("GPUI iOS: No windows registered");
    std::ptr::null_mut()
}

/// Store the finish launching callback.
///
/// This is called internally by IosPlatform::run() to store the callback
/// that will be invoked when the app finishes launching.
///
/// # Safety
/// This must only be called from the main thread.
pub(crate) fn set_finish_launching_callback(callback: Box<dyn FnOnce()>) {
    if let Some(state) = IOS_APP_STATE.get() {
        // Safety: Only called from main thread
        unsafe {
            *state.finish_launching.get() = Some(callback);
        }
    }
}

/// Called when the iOS app has finished launching.
///
/// This should be called from `application:didFinishLaunchingWithOptions:`
/// in the iOS app delegate, after `gpui_ios_initialize()` returns.
///
/// This invokes the callback passed to Application::run().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_did_finish_launching(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Did finish launching");

    if let Some(state) = IOS_APP_STATE.get() {
        // Safety: Only called from main thread
        let callback = unsafe { (*state.finish_launching.get()).take() };
        if let Some(callback) = callback {
            log::info!("GPUI iOS: Invoking finish launching callback");
            callback();
        } else {
            log::warn!("GPUI iOS: No finish launching callback registered");
        }
    } else {
        log::error!("GPUI iOS: Not initialized");
    }
}

/// Called when the iOS app will enter the foreground.
///
/// This should be called from `applicationWillEnterForeground:` in the app delegate.
/// This notifies all GPUI windows that the app is becoming active.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_will_enter_foreground(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Will enter foreground");

    // Notify all windows that they're becoming active
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            let windows = &*wrapper.0.get();
            for &window_ptr in windows.iter() {
                if !window_ptr.is_null() {
                    let window = &*window_ptr;
                    window.notify_active_status_change(true);
                }
            }
        }
    }
}

/// Called when the iOS app did become active.
///
/// This should be called from `applicationDidBecomeActive:` in the app delegate.
/// This indicates the app is now in the foreground and receiving events.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_did_become_active(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Did become active");

    // App is now fully active - windows should be notified
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            let windows = &*wrapper.0.get();
            for &window_ptr in windows.iter() {
                if !window_ptr.is_null() {
                    let window = &*window_ptr;
                    window.notify_active_status_change(true);
                }
            }
        }
    }
}

/// Called when the iOS app will resign active.
///
/// This should be called from `applicationWillResignActive:` in the app delegate.
/// This indicates the app is about to become inactive (e.g., incoming call, switching apps).
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_will_resign_active(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Will resign active");

    // App is about to become inactive
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            let windows = &*wrapper.0.get();
            for &window_ptr in windows.iter() {
                if !window_ptr.is_null() {
                    let window = &*window_ptr;
                    window.notify_active_status_change(false);
                }
            }
        }
    }
}

/// Called when the iOS app did enter the background.
///
/// This should be called from `applicationDidEnterBackground:` in the app delegate.
/// At this point, the app should have already saved any user data and released
/// shared resources. The app will be suspended shortly after this returns.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_did_enter_background(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Did enter background");

    // Notify windows they're no longer visible
    if let Some(wrapper) = IOS_WINDOW_LIST.get() {
        unsafe {
            let windows = &*wrapper.0.get();
            for &window_ptr in windows.iter() {
                if !window_ptr.is_null() {
                    let window = &*window_ptr;
                    window.notify_active_status_change(false);
                }
            }
        }
    }
}

/// Called when the iOS app will terminate.
///
/// This should be called from `applicationWillTerminate:` in the app delegate.
/// This is a good place to save any unsaved data.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_will_terminate(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Will terminate");

    // TODO: Could invoke quit callbacks here if needed
}

/// Called when a touch event occurs.
///
/// This bridges UIKit touch events to GPUI's input system.
/// Parameters:
/// - `window_ptr`: Pointer to the IosWindow
/// - `touch_ptr`: Pointer to the UITouch object
/// - `event_ptr`: Pointer to the UIEvent object
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_touch(
    window_ptr: *mut c_void,
    touch_ptr: *mut c_void,
    event_ptr: *mut c_void,
) {
    if window_ptr.is_null() || touch_ptr.is_null() {
        return;
    }

    // Cast to IosWindow and forward the touch event
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_touch(
        touch_ptr as *mut objc::runtime::Object,
        event_ptr as *mut objc::runtime::Object,
    );
}

/// Request a frame to be rendered.
///
/// This should be called from CADisplayLink callback to trigger GPUI rendering.
/// The window_ptr should be the value returned by gpui_ios_get_window().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_request_frame(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    // Safety: window_ptr must be a valid pointer to an IosWindow
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };

    // Advance fling before the render callback so scroll events are processed
    // in the same frame they are generated.
    window.process_fling();
    #[cfg(feature = "devtool")]
    window.process_devtool_gestures();

    let callback = window.request_frame_callback.borrow_mut().take();
    if let Some(mut cb) = callback {
        cb(RequestFrameOptions::default());
        window.request_frame_callback.borrow_mut().replace(cb);
    }
}

/// Request a frame to be rendered unconditionally (force_render = true).
///
/// Use this when there is known pending state (e.g. terminal output, callbacks)
/// that requires GPUI to re-render even if no views have been explicitly notified.
/// Mirrors Android's `platform.request_frame_forced()`.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_request_frame_forced(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };

    window.process_fling();

    let callback = window.request_frame_callback.borrow_mut().take();
    if let Some(mut cb) = callback {
        cb(RequestFrameOptions {
            force_render: true,
            ..Default::default()
        });
        window.request_frame_callback.borrow_mut().replace(cb);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_inject_scroll(
    window_ptr: *mut c_void,
    origin_x: f32,
    origin_y: f32,
    delta_x: f32,
    delta_y: f32,
    velocity_x: f32,
    velocity_y: f32,
    phase: i32,
) {
    if window_ptr.is_null() {
        return;
    }

    let touch_phase = match phase {
        2 => TouchPhase::Ended,
        _ => TouchPhase::Moved,
    };

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.inject_scroll(
        point(px(origin_x), px(origin_y)),
        point(px(delta_x), px(delta_y)),
        point(px(velocity_x), px(velocity_y)),
        touch_phase,
    );
}

/// Returns true when UIKit reports the software keyboard is visible.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_is_keyboard_visible(window_ptr: *mut c_void) -> bool {
    if window_ptr.is_null() {
        return false;
    }
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.is_software_keyboard_visible()
}

/// Show the software keyboard.
///
/// Call this when a text input field gains focus.
/// The window_ptr should be the value returned by gpui_ios_get_window().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_show_keyboard(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.show_soft_keyboard();
}

/// Set the UIView to display above the software keyboard (inputAccessoryView).
///
/// Pass a pointer to a UIView (e.g., UIToolbar) obtained from Objective-C.
/// The pointer is stored globally and returned by `inputAccessoryView` on GPUIMetalView.
/// Pass null to remove the accessory view.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_set_keyboard_accessory_view(view_ptr: *mut c_void) {
    super::window::set_keyboard_accessory_view(view_ptr);
}

/// Dispatch an action from the app-provided keyboard accessory view to the active input handler.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_keyboard_accessory_action(
    window_ptr: *mut c_void,
    action: *const c_char,
) -> bool {
    if window_ptr.is_null() || action.is_null() {
        return false;
    }
    let Ok(action) = (unsafe { CStr::from_ptr(action) }).to_str() else {
        return false;
    };
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_keyboard_accessory_action(action)
}

/// Dispatch a key-bar action that is not tied to an active keyboard session.
///
/// Same input-handler routing as `gpui_ios_handle_keyboard_accessory_action`,
/// minus the requirement that the software keyboard be up.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_key_bar_action(
    window_ptr: *mut c_void,
    action: *const c_char,
) -> bool {
    if window_ptr.is_null() || action.is_null() {
        return false;
    }
    let Ok(action) = (unsafe { CStr::from_ptr(action) }).to_str() else {
        return false;
    };
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_key_bar_action(action)
}

/// Insert composed text from a key bar into the focused input handler.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_key_bar_text(
    window_ptr: *mut c_void,
    text: *const c_char,
) -> bool {
    if window_ptr.is_null() || text.is_null() {
        return false;
    }
    let Ok(text) = (unsafe { CStr::from_ptr(text) }).to_str() else {
        return false;
    };
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_key_bar_text(text)
}

/// Track whether the iOS software keyboard is currently visible.
///
/// Called from ObjC keyboard notifications (keyboardWillShow / keyboardWillHide).
/// Controls whether `inputAccessoryView` returns the toolbar or nil.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_set_software_keyboard_visible(visible: bool) {
    super::window::set_software_keyboard_visible(visible);
}

/// Hide the software keyboard.
///
/// Call this when a text input field loses focus.
/// The window_ptr should be the value returned by gpui_ios_get_window().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_hide_keyboard(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.hide_soft_keyboard();
}

/// Handle text input from the software keyboard.
///
/// This is called when the user types on the keyboard.
/// Parameters:
/// - `window_ptr`: Pointer to the IosWindow
/// - `text_ptr`: Pointer to NSString with the entered text
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_text_input(window_ptr: *mut c_void, text_ptr: *mut c_void) {
    if window_ptr.is_null() || text_ptr.is_null() {
        return;
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_text_input(text_ptr as *mut objc::runtime::Object);
}

/// Handle a key event from an external keyboard.
///
/// Parameters:
/// - `window_ptr`: Pointer to the IosWindow
/// - `key_code`: The key code from UIKeyboardHIDUsage
/// - `modifiers`: Modifier flags from UIKeyModifierFlags
/// - `is_key_down`: true for key down, false for key up
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_key_event(
    window_ptr: *mut c_void,
    key_code: u32,
    modifiers: u32,
    is_key_down: bool,
) {
    if window_ptr.is_null() {
        return;
    }

    log::info!(
        "GPUI iOS: Handle key event - code: {}, modifiers: {}, down: {}",
        key_code,
        modifiers,
        is_key_down
    );

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_key_event(key_code, modifiers, is_key_down);
}

/// Notify the GPUI window that the screen size has changed (rotation or split-screen resize).
///
/// `width_pts` and `height_pts` are the new dimensions in logical points, as reported by
/// `[UIScreen mainScreen].bounds.size` after the orientation change.
/// This updates the window's logical bounds, resizes the Metal drawable, and fires the
/// GPUI resize callback so layout is recomputed at the new dimensions.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_view_resize(
    window_ptr: *mut c_void,
    width_pts: f32,
    height_pts: f32,
) {
    if window_ptr.is_null() {
        return;
    }
    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.handle_resize(width_pts, height_pts);
}

/// Get the UIWindow from a GPUI window.
///
/// # Safety
/// The window_ptr must be a valid pointer to an IosWindow.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_get_ui_window(
    window_ptr: *mut std::ffi::c_void,
) -> *mut std::ffi::c_void {
    if window_ptr.is_null() {
        return std::ptr::null_mut();
    }

    let window = unsafe { &*(window_ptr as *const super::window::IosWindow) };
    window.ui_window() as *mut std::ffi::c_void
}
