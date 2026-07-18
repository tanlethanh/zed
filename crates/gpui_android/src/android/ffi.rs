//! Android FFI: JNI bindings for `dev.zed.gpui.GpuiSurfaceView` and
//! `dev.zed.gpui.GpuiRuntimeController`, plus a small Rust-facing `run()`
//! helper that mirrors `gpui_ios::ios::ffi`.
//!
//! All entry points run on the Android main UI thread.

use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jdoubleArray, jfloat, jint, jlong, jstring};
use ndk::native_window::NativeWindow;

use gpui::{KeyDownEvent, Keystroke, Modifiers, PlatformInput};

use super::app_state;

// Selection FFI is shared by every GPUI surface (root, embedded sheet, future
// views). Each call carries the surface's opaque window handle so it routes to
// the right GPUI window; negative values clamp to the root handle.
fn selection_handle(window_handle: jlong) -> u64 {
    window_handle.max(0) as u64
}

// ===== Process-wide runtime state =====

static KEYBOARD_HEIGHT: AtomicU32 = AtomicU32::new(0);
static SYSTEM_INSET_TOP: AtomicU32 = AtomicU32::new(0);
static SYSTEM_INSET_BOTTOM: AtomicU32 = AtomicU32::new(0);
static DISPLAY_SCALE_BITS: AtomicU32 = AtomicU32::new(0);

const KEYCODE_DEL: i32 = 67;
const KEYCODE_FORWARD_DEL: i32 = 112;
const KEYCODE_ENTER: i32 = 66;
const KEYCODE_TAB: i32 = 61;
const KEYCODE_SPACE: i32 = 62;
const KEYCODE_ESCAPE: i32 = 111;
const KEYCODE_DPAD_UP: i32 = 19;
const KEYCODE_DPAD_DOWN: i32 = 20;
const KEYCODE_DPAD_LEFT: i32 = 21;
const KEYCODE_DPAD_RIGHT: i32 = 22;

/// Current Android soft-keyboard height in physical pixels (0 = hidden).
pub fn keyboard_height() -> u32 {
    KEYBOARD_HEIGHT.load(Ordering::Relaxed)
}

/// Current top system inset (status bar) in physical pixels.
pub fn system_inset_top() -> u32 {
    SYSTEM_INSET_TOP.load(Ordering::Relaxed)
}

/// Current bottom system inset (navigation bar) in physical pixels.
pub fn system_inset_bottom() -> u32 {
    SYSTEM_INSET_BOTTOM.load(Ordering::Relaxed)
}

/// Display scale factor reported by `Resources.DisplayMetrics.density`. Falls
/// back to `3.0` until Kotlin calls [`setDisplayScale`](#) on the runtime
/// controller.
pub fn display_scale() -> f32 {
    let bits = DISPLAY_SCALE_BITS.load(Ordering::Relaxed);
    if bits == 0 { 3.0 } else { f32::from_bits(bits) }
}

/// Run `f` against the registered [`AndroidPlatform`], returning `None` if no
/// platform has been created (i.e. before [`create_platform`] runs).
pub fn with_platform<R>(f: impl FnOnce(&Rc<super::platform::AndroidPlatform>) -> R) -> Option<R> {
    app_state::with_platform(f)
}

// ===== Public Rust API =====

/// Construct the [`AndroidPlatform`] and register it with the framework's FFI
/// layer so JNI callbacks (touch, surface, IME, lifecycle) can reach it.
///
/// Mirrors the role of `IosPlatform::new()` on iOS. Must be called after
/// `gpuiInit` has stored the JVM and Activity. The returned `Rc` is the only
/// strong handle to the platform — keep it alive (typically by passing into
/// `App::new_app`).
pub fn create_platform() -> Rc<super::platform::AndroidPlatform> {
    let (jvm, activity) = app_state::take_jvm_and_activity().expect(
        "gpui_android::create_platform: gpuiInit must run before create_platform() — \
         JVM/activity not stored",
    );
    let platform = Rc::new(super::platform::AndroidPlatform::new(jvm, activity));
    // setDisplayScale may have been pushed from Kotlin before the platform
    // existed — apply the cached value now so the first window opens at the
    // device's true density.
    platform.set_display_scale(display_scale());
    app_state::set_platform(platform.clone());
    platform
}

// ===== JNI exports — GpuiRuntimeController =====

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiInit(
    env: JNIEnv,
    _class: JClass,
    activity: JObject,
) {
    let jvm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(error) => {
            log::error!("gpui_android: failed to obtain JavaVM: {error:?}");
            return;
        }
    };
    let activity_ref = match env.new_global_ref(&activity) {
        Ok(r) => r,
        Err(error) => {
            log::error!("gpui_android: failed to create activity GlobalRef: {error:?}");
            return;
        }
    };

    app_state::store_jvm_and_activity(jvm, activity_ref);
    log::info!("gpui_android: gpuiInit complete");
}

/// Mark "the host has finished launching". On Android this is a no-op — the
/// finish-launching callback fires automatically the first time a surface is
/// attached, since `SurfaceView` arrives asynchronously and GPUI's first draw
/// can only succeed once the GPU surface is bound. Kept as an exposed JNI
/// symbol to keep the iOS-style API shape; safe to call (or omit) from Kotlin.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiDidFinishLaunching(
    _env: JNIEnv,
    _class: JClass,
) {
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiResume(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::with_platform(|platform| {
        platform.set_app_active(true);
        platform.request_frame_forced();
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiPause(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::with_platform(|platform| {
        platform.set_app_active(false);
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiDestroy(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::clear_platform();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiRequestFrame(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::with_platform(|platform| {
        platform.process_pending_tasks();
        platform.process_fling();
        #[cfg(feature = "devtool")]
        platform.process_devtool_gestures();
        // See: docs/GPUI_ANDROID_PERFORMANCE.md § fling-no-force
        platform.request_frame_for_all_windows();
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_gpuiRequestFrameForced(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::with_platform(|platform| {
        platform.process_pending_tasks();
        platform.request_frame_forced();
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiRuntimeController_setDisplayScale(
    _env: JNIEnv,
    _class: JClass,
    scale: jfloat,
) {
    DISPLAY_SCALE_BITS.store(scale.to_bits(), Ordering::Relaxed);
    app_state::with_platform(|platform| {
        platform.set_display_scale(scale);
    });
}

// ===== JNI exports — GpuiSurfaceView =====

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeSurfaceCreated(
    env: JNIEnv,
    _class: JClass,
    surface: JObject,
) {
    let native_window = match unsafe { NativeWindow::from_surface(env.get_raw(), surface.as_raw()) }
    {
        Some(w) => w,
        None => {
            log::error!("gpui_android: failed to obtain ANativeWindow from Surface");
            return;
        }
    };
    let width = native_window.width() as u32;
    let height = native_window.height() as u32;

    // Step 1: stash the native window (or attach to an existing GPUI window).
    if let Some(result) =
        app_state::with_platform(|platform| platform.attach_native_window(native_window))
    {
        if let Err(error) = result {
            log::error!("gpui_android: attach_native_window failed: {error:?}");
            return;
        }
    } else {
        log::error!("gpui_android: attach_native_window with no platform");
        return;
    }

    // Step 2: fire the finish-launching callback now that the surface is
    // available. `Application::run` stored the callback during `create_platform`;
    // on Android we cannot fire it earlier because `SurfaceView` is async and
    // GPUI's first draw requires the WGPU atlas to be bound. Fires once.
    if let Some(callback) = app_state::take_finish_launching_callback() {
        callback();
    }

    // Step 3: now the GPUI window exists, resize and force a redraw so the
    // first frame fills the actual surface dimensions.
    if let Some(result) =
        app_state::with_platform(|platform| platform.handle_surface_resize(width, height))
    {
        if let Err(error) = result {
            log::error!("gpui_android: handle_surface_resize failed: {error:?}");
        }
    }
    app_state::with_platform(|platform| platform.request_frame_forced());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeSurfaceChanged(
    _env: JNIEnv,
    _class: JClass,
    _format: jint,
    width: jint,
    height: jint,
) {
    let width = width.max(0) as u32;
    let height = height.max(0) as u32;
    if let Some(result) =
        app_state::with_platform(|platform| platform.handle_surface_resize(width, height))
    {
        if let Err(error) = result {
            log::error!("gpui_android: surface changed failed: {error:?}");
        }
    }
    app_state::with_platform(|platform| platform.request_frame_forced());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeSurfaceDestroyed(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::with_platform(|platform| platform.detach_native_window());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeTouchEvent(
    _env: JNIEnv,
    _class: JClass,
    action: jint,
    x: jfloat,
    y: jfloat,
    _pointer_id: jint,
) {
    app_state::with_platform(|platform| platform.handle_touch(action, x, y));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeFlingEvent(
    _env: JNIEnv,
    _class: JClass,
    velocity_x: jfloat,
    velocity_y: jfloat,
) {
    app_state::with_platform(|platform| platform.handle_fling(velocity_x, velocity_y));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeLongPressEvent(
    _env: JNIEnv,
    _class: JClass,
    x: jfloat,
    y: jfloat,
) {
    app_state::with_platform(|platform| platform.handle_long_press(x, y));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeKeyEvent(
    _env: JNIEnv,
    _class: JClass,
    action: jint,
    key_code: jint,
    unicode: jint,
) {
    // Only key-down for now; matches the prior Zedra behavior.
    if action != 0 {
        return;
    }
    if dispatch_text_input_key(key_code, unicode) {
        return;
    }
    let Some(keystroke) = android_keycode_to_keystroke(key_code, unicode) else {
        return;
    };
    let input = PlatformInput::KeyDown(KeyDownEvent {
        keystroke,
        is_held: false,
        prefer_character_input: false,
    });
    app_state::with_platform(|platform| {
        platform.dispatch_input(input);
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeImeInput(
    mut env: JNIEnv,
    _class: JClass,
    text: JString,
) {
    let text: String = match env.get_string(&text) {
        Ok(s) => s.into(),
        Err(error) => {
            log::error!("gpui_android: failed to read IME text: {error:?}");
            return;
        }
    };

    if app_state::with_platform(|platform| platform.insert_text(&text)).unwrap_or(false) {
        return;
    }

    app_state::with_platform(|platform| {
        for ch in text.chars() {
            let keystroke = Keystroke {
                modifiers: Modifiers::default(),
                key: ch.to_lowercase().to_string(),
                key_char: Some(ch.to_string()),
            };
            platform.dispatch_input(PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: false,
                prefer_character_input: true,
            }));
        }
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeImeSetComposingText(
    mut env: JNIEnv,
    _class: JClass,
    text: JString,
    new_cursor_position: jint,
) {
    let text: String = match env.get_string(&text) {
        Ok(s) => s.into(),
        Err(error) => {
            log::error!("gpui_android: failed to read composing text: {error:?}");
            return;
        }
    };

    app_state::with_platform(|platform| {
        platform.set_composing_text(&text, new_cursor_position);
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeImeFinishComposingText(
    _env: JNIEnv,
    _class: JClass,
) {
    app_state::with_platform(|platform| {
        platform.finish_composing_text();
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeKeyboardHeightChanged(
    _env: JNIEnv,
    _class: JClass,
    height: jint,
) {
    KEYBOARD_HEIGHT.store(height.max(0) as u32, Ordering::Relaxed);
    app_state::with_platform(|platform| platform.request_frame_forced());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_GpuiSurfaceView_nativeSystemInsetsChanged(
    _env: JNIEnv,
    _class: JClass,
    top: jint,
    bottom: jint,
) {
    SYSTEM_INSET_TOP.store(top.max(0) as u32, Ordering::Relaxed);
    SYSTEM_INSET_BOTTOM.store(bottom.max(0) as u32, Ordering::Relaxed);
    app_state::with_platform(|platform| platform.request_frame_forced());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionStartAt(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
    x: jfloat,
    y: jfloat,
) -> jint {
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| platform.start_selection_at(window_handle, x, y))
        .flatten()
        .map_or(-1, |index| index as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionNearestIndexAt(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
    x: jfloat,
    y: jfloat,
) -> jint {
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| platform.nearest_selection_index(window_handle, x, y))
        .flatten()
        .map_or(-1, |index| index as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionSetRange(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
    start: jint,
    end: jint,
) -> jboolean {
    if start < 0 || end < 0 {
        return false as jboolean;
    }
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| {
        platform.update_active_selection(window_handle, start as usize, end as usize)
    })
    .unwrap_or(false) as jboolean
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionTextForRange(
    env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
    start: jint,
    end: jint,
) -> jstring {
    if start < 0 || end < 0 {
        return std::ptr::null_mut();
    }
    let window_handle = selection_handle(window_handle);
    let Some(text) = app_state::with_platform(|platform| {
        platform.selection_text_for_range(window_handle, start as usize, end as usize)
    })
    .flatten() else {
        return std::ptr::null_mut();
    };
    let Ok(value) = env.new_string(text) else {
        return std::ptr::null_mut();
    };
    value.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionSnapshot(
    env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
) -> jdoubleArray {
    let window_handle = selection_handle(window_handle);
    let Some(snapshot) =
        app_state::with_platform(|platform| platform.active_selection_snapshot(window_handle))
            .flatten()
    else {
        return std::ptr::null_mut();
    };
    let Ok(array) = env.new_double_array(snapshot.len() as jint) else {
        return std::ptr::null_mut();
    };
    if env.set_double_array_region(&array, 0, &snapshot).is_err() {
        return std::ptr::null_mut();
    }
    array.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionCopy(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
) -> jboolean {
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| platform.copy_active_selection(window_handle))
        .unwrap_or(false) as jboolean
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionText(
    env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
) -> jstring {
    let window_handle = selection_handle(window_handle);
    let Some(text) =
        app_state::with_platform(|platform| platform.selected_text(window_handle)).flatten()
    else {
        return std::ptr::null_mut();
    };
    let Ok(value) = env.new_string(text) else {
        return std::ptr::null_mut();
    };
    value.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionActionCount(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
) -> jint {
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| platform.selection_action_count(window_handle))
        .unwrap_or(0)
        .min(i32::MAX as usize) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionCustomActionsOnly(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
) -> jboolean {
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| platform.selection_custom_actions_only(window_handle))
        .unwrap_or(false) as jboolean
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionActionTitle(
    env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
    action_index: jint,
) -> jstring {
    if action_index < 0 {
        return std::ptr::null_mut();
    }
    let window_handle = selection_handle(window_handle);
    let Some(title) = app_state::with_platform(|platform| {
        platform.selection_action_title(window_handle, action_index as usize)
    })
    .flatten() else {
        return std::ptr::null_mut();
    };
    let Ok(value) = env.new_string(title) else {
        return std::ptr::null_mut();
    };
    value.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionPerformAction(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
    action_index: jint,
) -> jboolean {
    if action_index < 0 {
        return false as jboolean;
    }
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| {
        platform.perform_selection_action(window_handle, action_index as usize)
    })
    .unwrap_or(false) as jboolean
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_zed_gpui_SelectionController_nativeSelectionClear(
    _env: JNIEnv,
    _class: JClass,
    window_handle: jlong,
) {
    let window_handle = selection_handle(window_handle);
    app_state::with_platform(|platform| platform.clear_active_selection(window_handle));
}

// ===== Helpers =====

fn dispatch_text_input_key(key_code: i32, unicode: i32) -> bool {
    if key_code == KEYCODE_DEL {
        return app_state::with_platform(|platform| platform.delete_backward(1)).unwrap_or(false);
    }

    let text = match key_code {
        KEYCODE_ENTER => Some("\n".to_string()),
        KEYCODE_TAB => Some("\t".to_string()),
        _ if unicode > 0 => char::from_u32(unicode as u32).map(|ch| ch.to_string()),
        _ => None,
    };

    text.is_some_and(|text| {
        app_state::with_platform(|platform| platform.insert_text(&text)).unwrap_or(false)
    })
}

fn android_keycode_to_keystroke(key_code: i32, unicode: i32) -> Option<Keystroke> {
    let key = match key_code {
        KEYCODE_DEL => "backspace".to_string(),
        KEYCODE_FORWARD_DEL => "delete".to_string(),
        KEYCODE_ENTER => "enter".to_string(),
        KEYCODE_TAB => "tab".to_string(),
        KEYCODE_SPACE => "space".to_string(),
        KEYCODE_ESCAPE => "escape".to_string(),
        KEYCODE_DPAD_UP => "up".to_string(),
        KEYCODE_DPAD_DOWN => "down".to_string(),
        KEYCODE_DPAD_LEFT => "left".to_string(),
        KEYCODE_DPAD_RIGHT => "right".to_string(),
        _ => {
            if unicode > 0 {
                let ch = char::from_u32(unicode as u32)?;
                ch.to_lowercase().to_string()
            } else {
                return None;
            }
        }
    };

    let key_char = if unicode > 0 {
        char::from_u32(unicode as u32).map(|c| c.to_string())
    } else {
        None
    };

    Some(Keystroke {
        modifiers: Modifiers::default(),
        key,
        key_char,
    })
}
