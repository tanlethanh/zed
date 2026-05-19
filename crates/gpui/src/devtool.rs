//! In-app devtool registry. Exposes a snapshot of the most recently painted
//! interactive hitboxes keyed by `ElementId` path so an external HTTP server
//! (see `gpui_android::devtool_server`) can drive synthetic taps.
//!
//! Gated by the `devtool` cargo feature; intended for debug builds only.

use std::sync::Mutex;

use crate::{ElementId, GlobalElementId, Hitbox, InspectorElementId};

/// One interactive hitbox, snapshot-cloned from `next_frame.hitboxes` after
/// paint. Bounds are in device-independent pixels — clients apply scale factor
/// themselves when injecting platform input.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Slash-joined ElementId path from root to this element.
    pub path: String,
    /// `InspectorElementId.instance_id` so callers can disambiguate elements
    /// sharing the same path within one frame.
    pub instance_id: usize,
    /// Bounds origin x (logical pixels).
    pub x: f32,
    /// Bounds origin y.
    pub y: f32,
    /// Bounds width.
    pub width: f32,
    /// Bounds height.
    pub height: f32,
}

/// Latest frame's hitbox snapshot. Replaced wholesale at the end of paint.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Monotonic frame counter, mirrors `PerDebug` ids when both features on.
    pub frame_id: u64,
    /// All hitboxes that had an associated `InspectorElementId` this frame.
    pub entries: Vec<Entry>,
}

static REGISTRY: Mutex<Snapshot> = Mutex::new(Snapshot {
    frame_id: 0,
    entries: Vec::new(),
});

/// Replace the snapshot with the latest frame's hitboxes.
pub fn publish(frame_id: u64, hitboxes: &[Hitbox], ids: &collections::FxHashMap<crate::HitboxId, InspectorElementId>) {
    let mut entries = Vec::with_capacity(ids.len());
    for hitbox in hitboxes {
        let Some(inspector_id) = ids.get(&hitbox.id) else {
            continue;
        };
        let path = format_path(&inspector_id.path.global_id);
        entries.push(Entry {
            path,
            instance_id: inspector_id.instance_id,
            x: f32::from(hitbox.bounds.origin.x),
            y: f32::from(hitbox.bounds.origin.y),
            width: f32::from(hitbox.bounds.size.width),
            height: f32::from(hitbox.bounds.size.height),
        });
    }
    if let Ok(mut guard) = REGISTRY.lock() {
        *guard = Snapshot { frame_id, entries };
    }
}

/// Clone the most recently published snapshot. Returns an empty snapshot if the
/// app has not painted yet.
pub fn snapshot() -> Snapshot {
    REGISTRY
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

fn format_path(global_id: &GlobalElementId) -> String {
    let mut out = String::new();
    for (i, id) in global_id.0.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        format_element_id(id, &mut out);
    }
    out
}

fn format_element_id(id: &ElementId, out: &mut String) {
    use std::fmt::Write as _;
    match id {
        ElementId::View(entity_id) => {
            let _ = write!(out, "view#{}", entity_id);
        }
        ElementId::Integer(n) => {
            let _ = write!(out, "{}", n);
        }
        ElementId::Name(name) => out.push_str(name),
        ElementId::Uuid(uuid) => {
            let _ = write!(out, "uuid:{}", uuid);
        }
        ElementId::FocusHandle(_) => out.push_str("focus"),
        ElementId::NamedInteger(name, n) => {
            let _ = write!(out, "{}[{}]", name, n);
        }
        ElementId::Path(path) => {
            let _ = write!(out, "path:{}", path.display());
        }
        ElementId::CodeLocation(loc) => {
            let _ = write!(out, "loc:{}:{}", loc.file(), loc.line());
        }
        ElementId::NamedChild(parent, name) => {
            format_element_id(parent, out);
            out.push(':');
            out.push_str(name);
        }
        ElementId::OpaqueId(_) => out.push_str("opaque"),
    }
}
