//! In-app devtool registry. Exposes a snapshot of the most recently painted interactive
//! hitboxes keyed by `ElementId` path so an external HTTP server (`gpui_devtool` crate) can
//! drive synthetic taps. Gated by the `devtool` cargo feature; debug builds only.
//!
//! Snapshot is read from the HTTP server's own thread, so it must be `Send`/`Sync`.
//! Addressed by *path*, not `HitboxId`, since `HitboxId` is a per-paint counter reassigned every
//! frame — an early version queued by `HitboxId` and every lookup silently no-op'd.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use crate::{App, ElementId, Global, GlobalElementId, Hitbox, InspectorElementId, SharedString, Window};

/// Longest `snapshot()` blocks waiting for a fresh publish — generous headroom for a slow
/// render loop, not an expected-case duration (publish normally resolves within one frame tick).
const PUBLISH_WAIT_TIMEOUT: Duration = Duration::from_millis(2000);

/// One interactive hitbox, snapshot-cloned after paint. Bounds are logical pixels — clients
/// apply scale factor themselves when injecting platform input.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Slash-joined ElementId path from root to this element.
    pub path: String,
    /// Disambiguates elements sharing the same path within one frame.
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

/// Set by `snapshot()`, cleared by `publish()` — makes publishing pull-triggered instead of
/// unconditional every draw: rebuilding the snapshot (a `String` alloc per hitbox) is wasted
/// work on the many frames nobody's actually querying `/elements`, and this app has real
/// continuous-redraw periods (spinners, cursor blink) where "every draw" would mean "every
/// frame of that animation."
static NEEDS_PUBLISH: AtomicBool = AtomicBool::new(true);
/// Bumped each time `publish()` actually runs; lets `snapshot()` tell "a publish that started
/// after my request" apart from "the one that was already in flight when I asked."
static PUBLISH_GENERATION: AtomicU64 = AtomicU64::new(0);
static PUBLISH_LOCK: Mutex<()> = Mutex::new(());
static PUBLISH_CONDVAR: Condvar = Condvar::new();

/// Whether a fresh publish has been requested since the last one ran. Checked by the window's
/// per-tick dirty-gate so a request isn't stranded on an idle screen — `Window::draw` only
/// runs when dirty, and a queued request doesn't dirty anything on its own. Relies on the
/// platform tick loop running unconditionally while foregrounded, same as gesture draining
/// (see `enqueue_gesture` in `gpui_devtool.rs`).
pub fn has_pending_publish() -> bool {
    NEEDS_PUBLISH.load(Ordering::Relaxed)
}

/// Replace the snapshot with the latest frame's hitboxes — but only if `snapshot()` actually
/// asked for one since the last call (see `NEEDS_PUBLISH`); otherwise a no-op.
pub fn publish(
    frame_id: u64,
    hitboxes: &[Hitbox],
    ids: &collections::FxHashMap<crate::HitboxId, InspectorElementId>,
) {
    if !NEEDS_PUBLISH.swap(false, Ordering::Relaxed) {
        return;
    }
    let mut entries = Vec::with_capacity(ids.len());
    for hitbox in hitboxes {
        let Some(inspector_id) = ids.get(&hitbox.id) else {
            continue;
        };
        entries.push(Entry {
            path: format_path(&inspector_id.path.global_id),
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
    PUBLISH_GENERATION.fetch_add(1, Ordering::Relaxed);
    if let Ok(guard) = PUBLISH_LOCK.lock() {
        PUBLISH_CONDVAR.notify_all();
        drop(guard);
    }
}

/// Request a fresh publish and block (bounded timeout) until one actually happens, then clone
/// it. Returns an empty snapshot if the app has not painted yet and the wait times out.
pub fn snapshot() -> Snapshot {
    let seen_generation = PUBLISH_GENERATION.load(Ordering::Relaxed);
    NEEDS_PUBLISH.store(true, Ordering::Relaxed);
    if let Ok(guard) = PUBLISH_LOCK.lock() {
        let _ = PUBLISH_CONDVAR.wait_timeout_while(guard, PUBLISH_WAIT_TIMEOUT, |_| {
            PUBLISH_GENERATION.load(Ordering::Relaxed) <= seen_generation
        });
    }
    REGISTRY
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

pub(crate) fn format_path(global_id: &GlobalElementId) -> String {
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

/// Debug-only functions registered by app code, invokable via `POST /call {"name","params"}` —
/// not tied to any UI element, so registering one doesn't need an `.id(...)`'d div at all.
/// Meant to be added while debugging a specific feature and deleted when done (register once,
/// e.g. in an entity's constructor, capturing whatever it needs); `App::register_devtool_action`
/// always compiles and no-ops when the `devtool` feature is off, so the call site never needs
/// `#[cfg(...)]`.
#[derive(Default)]
pub(crate) struct ActionRegistry {
    pub(crate) functions:
        HashMap<SharedString, Rc<dyn Fn(serde_json::Value, &mut Window, &mut App) -> serde_json::Value>>,
}

impl Global for ActionRegistry {}

/// Lets a registered function return either `()` (pure side effect, reported as `Value::Null`)
/// or a real `serde_json::Value` the caller can inspect — so a `/call` that doesn't need to
/// report anything back doesn't have to end with an explicit `Value::Null`.
pub trait IntoDevtoolResult {
    /// Convert into the `Value` sent back as `/call`'s `"result"` field.
    fn into_devtool_result(self) -> serde_json::Value;
}

impl IntoDevtoolResult for () {
    fn into_devtool_result(self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

impl IntoDevtoolResult for serde_json::Value {
    fn into_devtool_result(self) -> serde_json::Value {
        self
    }
}

impl App {
    /// Register a debug-only function under `name`, invokable via `POST /call`. Registering the
    /// same name twice replaces the previous closure. `f` can return `()` for a pure side effect
    /// or a `serde_json::Value` the caller can check — see `IntoDevtoolResult`/`ActionRegistry`.
    #[cfg(feature = "devtool")]
    pub fn register_devtool_action<R: IntoDevtoolResult>(
        &mut self,
        name: impl Into<SharedString>,
        f: impl Fn(serde_json::Value, &mut Window, &mut App) -> R + 'static,
    ) {
        self.default_global::<ActionRegistry>().functions.insert(
            name.into(),
            Rc::new(move |params, window, cx| f(params, window, cx).into_devtool_result()),
        );
    }

    #[cfg(not(feature = "devtool"))]
    pub fn register_devtool_action<R: IntoDevtoolResult>(
        &mut self,
        _name: impl Into<SharedString>,
        _f: impl Fn(serde_json::Value, &mut Window, &mut App) -> R + 'static,
    ) {
    }
}

struct PendingCall {
    id: u64,
    name: String,
    params: serde_json::Value,
}

/// Result of waiting for a `POST /call` to run.
pub struct CallOutcome {
    /// Whether `Window::draw` processed this request within the timeout at all.
    pub completed: bool,
    /// Whether `name` was actually registered — `false` if nothing was ever registered under
    /// that name, distinct from a timeout (`completed: false`).
    pub invoked: bool,
    /// The registered function's return value; `Value::Null` if `invoked` is `false`.
    pub result: serde_json::Value,
}

/// The `/call` request queue crossing the devtool HTTP thread -> main GPUI thread boundary.
/// Only plain data (id/name/params) crosses here — the registered closure stays in the
/// main-thread-only `ActionRegistry` global, looked up fresh by name each time (see
/// `Window::invoke_devtool_call`). One `LazyLock<CallQueue>` instead of separate statics for
/// the queue, id counter, results, and condvar, since they're never meaningfully used apart.
struct CallQueue {
    pending: Mutex<VecDeque<PendingCall>>,
    next_id: AtomicU64,
    results: Mutex<HashMap<u64, (bool, serde_json::Value)>>,
    condvar: Condvar,
}

impl CallQueue {
    fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            next_id: AtomicU64::new(1),
            results: Mutex::new(HashMap::new()),
            condvar: Condvar::new(),
        }
    }

    fn enqueue(&self, name: String, params: serde_json::Value) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut pending) = self.pending.lock() {
            pending.push_back(PendingCall { id, name, params });
        }
        id
    }

    fn drain(&self) -> Vec<(u64, String, serde_json::Value)> {
        self.pending
            .lock()
            .map(|mut pending| pending.drain(..).map(|c| (c.id, c.name, c.params)).collect())
            .unwrap_or_default()
    }

    fn mark(&self, id: u64, invoked: bool, result: serde_json::Value) {
        if let Ok(mut results) = self.results.lock() {
            results.insert(id, (invoked, result));
        }
        self.condvar.notify_all();
    }

    fn wait(&self, id: u64) -> CallOutcome {
        let no_result = CallOutcome {
            completed: false,
            invoked: false,
            result: serde_json::Value::Null,
        };
        let Ok(guard) = self.results.lock() else {
            return no_result;
        };
        let wait = self
            .condvar
            .wait_timeout_while(guard, PUBLISH_WAIT_TIMEOUT, |results| {
                !results.contains_key(&id)
            });
        let Ok((mut results, wait_result)) = wait else {
            return no_result;
        };
        if wait_result.timed_out() {
            return no_result;
        }
        let (invoked, result) = results.remove(&id).unwrap_or_default();
        CallOutcome {
            completed: true,
            invoked,
            result,
        }
    }

    fn has_pending(&self) -> bool {
        self.pending.lock().map(|pending| !pending.is_empty()).unwrap_or(false)
    }
}

static CALLS: LazyLock<CallQueue> = LazyLock::new(CallQueue::new);

/// Enqueue a call (from the devtool HTTP thread) to run on the next `Window::draw`.
pub fn enqueue_call(name: String, params: serde_json::Value) -> u64 {
    CALLS.enqueue(name, params)
}

/// Drain calls queued since the last call. Called once per frame from `Window::draw`.
pub fn drain_pending_calls() -> Vec<(u64, String, serde_json::Value)> {
    CALLS.drain()
}

/// Record request `id`'s result. Called once per drained call from `Window::draw`.
pub fn mark_call_result(id: u64, invoked: bool, result: serde_json::Value) {
    CALLS.mark(id, invoked, result);
}

/// Block (bounded timeout) until request `id` has been processed by `Window::draw`.
pub fn wait_for_call(id: u64) -> CallOutcome {
    CALLS.wait(id)
}

/// Whether a call is waiting to run — same reasoning as `has_pending_publish`: `Window::draw`
/// only runs when dirty, and a queued call doesn't dirty anything on its own.
pub fn has_pending_calls() -> bool {
    CALLS.has_pending()
}
