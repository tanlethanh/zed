#![cfg(target_os = "ios")]
#![allow(dead_code)]
//! iOS platform implementation for GPUI.
//!
//! This crate provides the iOS platform backend for GPUI, using Metal
//! for GPU rendering and UIKit for windowing and input.

mod ios;
mod metal_atlas;
pub mod metal_renderer;
pub mod render_effect;

pub use ios::ffi;
pub use ios::platform::IosPlatform;
pub use render_effect::{IosRenderEffect, MetalEffectContext, MetalRenderEffect};
