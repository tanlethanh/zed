#![cfg(target_os = "ios")]
//! iOS platform implementation for GPUI.
//!
//! This crate provides the iOS platform backend for GPUI, using Metal
//! for GPU rendering and UIKit for windowing and input.

mod ios;
mod metal_atlas;
pub mod metal_renderer;

pub use ios::platform::IosPlatform;
pub use ios::ffi;
