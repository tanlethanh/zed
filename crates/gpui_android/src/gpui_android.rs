#![cfg(target_os = "android")]
mod android;

pub use android::dispatcher::AndroidQueueReceiver;
pub use android::platform::{AndroidCommon, AndroidPlatform};
pub use android::window::{AndroidWindow, AndroidWindowState};
