//! Synchronous wrapper around [`WgpuRenderer`].
//!
//! Previously this module ran an off-thread pipelined renderer worker, but
//! that required `Scene: Clone` and `GpuContext: Send + Sync` — both leaked
//! Android-specific changes into shared `gpui` / `gpui_wgpu` crates. The
//! pipelining was also the top spike source per
//! `docs/AUDIT_P50_OPTS_SPIKES.md` § pipelined-render-thread.
//!
//! For now the wrapper draws inline. The name is retained so call-sites in
//! `window.rs` stay stable.

use std::cell::{RefCell, RefMut};
use std::sync::Arc;

use gpui::Scene;
use gpui_wgpu::{WgpuAtlas, WgpuRenderer};

pub struct PipelinedRenderer {
    renderer: RefCell<WgpuRenderer>,
    atlas: Arc<WgpuAtlas>,
}

impl PipelinedRenderer {
    pub fn new(renderer: WgpuRenderer) -> Self {
        let atlas = renderer.atlas();
        Self {
            renderer: RefCell::new(renderer),
            atlas,
        }
    }

    pub fn lock(&self) -> RefMut<'_, WgpuRenderer> {
        self.renderer.borrow_mut()
    }

    pub fn atlas(&self) -> &Arc<WgpuAtlas> {
        &self.atlas
    }

    pub fn draw(&self, scene: &Scene) {
        self.renderer.borrow_mut().draw(scene);
    }
}
