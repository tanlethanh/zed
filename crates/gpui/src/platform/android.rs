mod dispatcher;
mod keyboard;
mod platform;
mod text_system;
mod window;

pub(crate) use dispatcher::*;
pub(crate) use keyboard::*;
pub(crate) use text_system::*;
pub(crate) use window::*;

// Export AndroidPlatform publicly for app integration
pub use platform::AndroidPlatform;
pub(crate) use platform::{AndroidClient, AndroidCommon};

// Screen capture not yet supported on Android
pub(crate) type PlatformScreenCaptureFrame = ();
