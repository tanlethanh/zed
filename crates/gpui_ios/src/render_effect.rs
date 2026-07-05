use gpui::{DevicePixels, Size};

/// Frame resources handed to a render effect after the scene has been encoded.
pub struct MetalEffectContext<'a> {
    pub device: &'a metal::DeviceRef,
    pub command_buffer: &'a metal::CommandBufferRef,
    pub drawable_texture: &'a metal::TextureRef,
    pub viewport_size: Size<DevicePixels>,
}

/// App-provided post-scene effect; `encode` runs per frame between the scene
/// batches and present, creating its own encoders on the command buffer.
pub trait MetalRenderEffect: 'static {
    fn encode(&mut self, cx: &MetalEffectContext);
}

/// Wrapper for `Window::set_render_effect`; `dyn Any` can't downcast to a trait object.
pub struct IosRenderEffect(pub Box<dyn MetalRenderEffect>);
