mod dispatcher;
mod keyboard;
mod platform;
mod window;

pub(crate) use dispatcher::*;
pub(crate) use keyboard::*;
pub(crate) use window::*;

// Export AndroidPlatform publicly for app integration
pub use platform::AndroidPlatform;
pub(crate) use platform::{AndroidClient, AndroidCommon};

// Screen capture not yet supported on Android
pub(crate) type PlatformScreenCaptureFrame = ();
