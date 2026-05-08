//! Process-wide state for the Android FFI layer.
//!
//! Mirrors `gpui_ios::ios::ffi::IOS_APP_STATE` and `IOS_WINDOW_LIST`. All access
//! happens on the Android main UI thread, so an `UnsafeCell` is sufficient — the
//! `Send`/`Sync` impls exist only to satisfy `OnceLock`'s bounds.

use std::cell::UnsafeCell;
use std::rc::Rc;
use std::sync::OnceLock;

use jni::JavaVM;
use jni::objects::GlobalRef;

use super::platform::AndroidPlatform;

struct AndroidAppState {
    finish_launching: UnsafeCell<Option<Box<dyn FnOnce()>>>,
    platform: UnsafeCell<Option<Rc<AndroidPlatform>>>,
    jvm: UnsafeCell<Option<JavaVM>>,
    activity: UnsafeCell<Option<GlobalRef>>,
}

// Safety: every `UnsafeCell` is only touched from the Android main UI thread.
// `OnceLock<T>` requires `T: Send + Sync` for use in a static.
unsafe impl Send for AndroidAppState {}
unsafe impl Sync for AndroidAppState {}

static ANDROID_APP_STATE: OnceLock<AndroidAppState> = OnceLock::new();

pub(crate) fn ensure_initialized() {
    let _ = ANDROID_APP_STATE.set(AndroidAppState {
        finish_launching: UnsafeCell::new(None),
        platform: UnsafeCell::new(None),
        jvm: UnsafeCell::new(None),
        activity: UnsafeCell::new(None),
    });
}

pub(crate) fn store_jvm_and_activity(jvm: JavaVM, activity: GlobalRef) {
    ensure_initialized();
    if let Some(state) = ANDROID_APP_STATE.get() {
        unsafe {
            *state.jvm.get() = Some(jvm);
            *state.activity.get() = Some(activity);
        }
    }
}

pub(crate) fn take_jvm_and_activity() -> Option<(JavaVM, GlobalRef)> {
    let state = ANDROID_APP_STATE.get()?;
    unsafe {
        let jvm = (*state.jvm.get()).take()?;
        let activity = (*state.activity.get()).take()?;
        Some((jvm, activity))
    }
}

pub(crate) fn set_platform(platform: Rc<AndroidPlatform>) {
    if let Some(state) = ANDROID_APP_STATE.get() {
        unsafe {
            *state.platform.get() = Some(platform);
        }
    }
}

pub(crate) fn with_platform<R>(f: impl FnOnce(&Rc<AndroidPlatform>) -> R) -> Option<R> {
    let state = ANDROID_APP_STATE.get()?;
    unsafe { (*state.platform.get()).as_ref().map(f) }
}

pub(crate) fn set_finish_launching_callback(callback: Box<dyn FnOnce()>) {
    ensure_initialized();
    if let Some(state) = ANDROID_APP_STATE.get() {
        unsafe {
            *state.finish_launching.get() = Some(callback);
        }
    }
}

pub(crate) fn take_finish_launching_callback() -> Option<Box<dyn FnOnce()>> {
    let state = ANDROID_APP_STATE.get()?;
    unsafe { (*state.finish_launching.get()).take() }
}

pub(crate) fn clear_platform() {
    if let Some(state) = ANDROID_APP_STATE.get() {
        unsafe {
            *state.platform.get() = None;
        }
    }
}
