# Android Support Implementation Plan for GPUI

**Status:** Planning Phase
**Target:** GPUI v0.3.0+
**Complexity:** High (estimated 3-6 months for MVP)
**Created:** 2026-01-07

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Feasibility Analysis](#feasibility-analysis)
3. [Reusable Components from Linux Implementation](#reusable-components-from-linux-implementation)
4. [Android-Specific Requirements](#android-specific-requirements)
5. [Architecture Overview](#architecture-overview)
6. [Implementation Phases](#implementation-phases)
7. [Technical Challenges and Solutions](#technical-challenges-and-solutions)
8. [Dependencies and Prerequisites](#dependencies-and-prerequisites)
9. [Testing Strategy](#testing-strategy)
10. [Performance Considerations](#performance-considerations)
11. [Open Questions](#open-questions)
12. [Resources and References](#resources-and-references)

---

## Executive Summary

This document outlines a comprehensive plan to add Android support to GPUI. The analysis shows that **significant portions of the Linux implementation can be reused**, particularly:

- **Blade renderer (Vulkan)** - 100% reusable
- **CosmicText text system** - 100% reusable
- **Core GPUI framework** - 100% reusable
- **Platform abstraction layer** - Requires Android-specific implementation

**Key Finding:** The Linux implementation using Blade (Vulkan) provides an excellent foundation for Android support, requiring primarily window management and event handling adaptations rather than a complete rewrite.

**Estimated Effort:**

- **Phase 1 (MVP):** 2-3 months - Basic window, rendering, input
- **Phase 2 (Complete):** 1-2 months - Full features, optimization
- **Phase 3 (Polish):** 1 month - Testing, performance, documentation

---

## Feasibility Analysis

### High Reusability Assessment

| Component         | Linux Implementation        | Android Reusability | Notes                                    |
| ----------------- | --------------------------- | ------------------- | ---------------------------------------- |
| Blade Renderer    | Vulkan via blade-graphics   | **100% reusable**   | Vulkan well-supported on Android API 24+ |
| CosmicText        | Pure Rust text system       | **100% reusable**   | No platform dependencies                 |
| Blade Atlas       | Texture atlas management    | **100% reusable**   | Platform-agnostic                        |
| Core Framework    | App/Context/Entity/Executor | **100% reusable**   | Platform-agnostic design                 |
| Geometry/Style    | Layout and styling          | **100% reusable**   | No platform code                         |
| Window Management | Wayland/X11 specific        | **0% reusable**     | Need Android-specific                    |
| Event Handling    | Linux event loop            | **30% reusable**    | Need Android lifecycle                   |
| Clipboard         | Linux clipboard APIs        | **0% reusable**     | Need Android ClipboardManager            |
| File Picker       | XDG Desktop Portal          | **0% reusable**     | Need Android intents                     |
| Keyboard Mapper   | XKB                         | **50% reusable**    | Android has different input system       |

**Overall Reusability:** ~70% of code reusable

### Technical Feasibility: HIGH ✅

**Strengths:**

- Vulkan support mature on Android (API 24+, ~99% devices)
- raw-window-handle has AndroidNdkWindowHandle support
- blade-graphics confirmed working on Android
- Recent success stories (iced-android-example, 2025)
- Pure Rust stack (cosmic-text, blade) reduces FFI complexity

**Challenges:**

- Android lifecycle complexity (Activity/Surface lifecycle)
- Soft keyboard management
- Permission system integration
- APK packaging and distribution
- Play Store requirements (if applicable)

---

## Reusable Components from Linux Implementation

### 1. Blade Renderer (src/platform/blade/)

**Files:** 5 files, ~1,666 lines
**Reusability:** 100%

**What can be reused:**

```rust
// blade_renderer.rs - Complete rendering pipeline
pub struct BladeRenderer {
    gpu: Arc<gpu::Context>,
    pipelines: BladePipelines,
    // ... all rendering logic
}
```

**Evidence from Linux implementation:**

```rust
// From blade_context.rs:12
impl BladeContext {
    pub fn new() -> anyhow::Result<Self> {
        let gpu = Arc::new(
            unsafe {
                gpu::Context::init(gpu::ContextDesc {
                    presentation: true,
                    validation: false,
                    device_id: device_id_forced.unwrap_or(0),
                    ..Default::default()
                })
            }
        );
        Ok(Self { gpu })
    }
}
```

This initialization is platform-agnostic and will work on Android with Vulkan support.

**Integration path:**

1. Use raw-window-handle's AndroidNdkWindowHandle
2. Initialize Blade context (unchanged)
3. Create surface from ANativeWindow
4. Use existing rendering pipeline

**Shader support:**

- WGSL shaders (shaders.wgsl) - Platform independent ✅
- Runtime compilation by blade-graphics ✅
- No platform-specific shader code needed ✅

### 2. CosmicText System (src/platform/linux/text_system.rs)

**Lines:** ~600 lines
**Reusability:** 100%

**Why it's perfect for Android:**

```rust
// From text_system.rs:27
pub(crate) struct CosmicTextSystem(RwLock<CosmicTextSystemState>);

struct CosmicTextSystemState {
    font_system: FontSystem,  // Pure Rust
    scratch: ShapeBuffer,     // Pure Rust
    swash_scale_context: ScaleContext,  // Pure Rust
    // No platform-specific code!
}
```

**Advantages:**

- No C/C++ dependencies (no FFI issues)
- Works with any font format
- Handles complex scripts (Arabic, Thai, etc.)
- Font fallback built-in
- Emoji support via font detection

**Android-specific additions needed:**

```rust
impl CosmicTextSystemState {
    fn load_android_system_fonts(&mut self) {
        // Load from /system/fonts/
        // Roboto, Noto families
    }
}
```

### 3. Platform Abstraction (platform.rs:184-200)

The existing `Platform` trait is well-designed for Android:

```rust
pub trait Platform: 'static {
    fn background_executor(&self) -> BackgroundExecutor;  // ✅ Reusable
    fn foreground_executor(&self) -> ForegroundExecutor;  // ✅ Reusable
    fn text_system(&self) -> Arc<dyn PlatformTextSystem>; // ✅ Reusable
    fn open_window(...) -> Box<dyn PlatformWindow>;       // ❌ Android-specific
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>>;   // ❌ Android-specific
    fn open_url(&self, url: &str);                        // ❌ Android-specific (Intent)
    fn write_to_clipboard(&self, item: ClipboardItem);    // ❌ Android-specific
    // ... more methods
}
```

**Reuse pattern:**

```rust
// From linux/platform.rs:184
impl<P: LinuxClient + 'static> Platform for P {
    fn background_executor(&self) -> BackgroundExecutor {
        self.with_common(|common| common.background_executor.clone())
    }
    // ... identical pattern works for Android
}
```

We can follow the same pattern with `AndroidClient` trait.

### 4. Core Framework (100% Reusable)

All core framework code is platform-agnostic:

- **app.rs** (91,738 lines) - Entity management, effects, observers
- **window.rs** (199,692 lines) - Window state, rendering coordination
- **executor.rs** (39,309 lines) - Async execution
- **element.rs** + **elements/** - All UI elements
- **style.rs** - Styling system
- **taffy.rs** - Layout engine

**Zero changes needed** for these components!

---

## Android-Specific Requirements

### 1. Window Management (NEW)

**File:** `src/platform/android/window.rs` (~1,500 lines estimated)

**Requirements:**

#### ANativeWindow Integration

```rust
use raw_window_handle::{AndroidNdkWindowHandle, RawWindowHandle};
use ndk::native_window::NativeWindow;

pub struct AndroidWindowState {
    // ANativeWindow wrapper
    native_window: Option<NativeWindow>,

    // Blade renderer (reusable!)
    renderer: BladeRenderer,

    // Window state
    bounds: Bounds<Pixels>,
    scale: f32,

    // Input handling
    input_handler: Option<PlatformInputHandler>,

    // Android-specific
    soft_keyboard_visible: bool,
    system_ui_visibility: SystemUiVisibility,
}
```

#### Lifecycle Management

**Critical Android requirement:**

```rust
impl AndroidWindow {
    fn handle_surface_created(&mut self, window: NativeWindow) {
        // ANativeWindow became available
        self.native_window = Some(window);
        self.initialize_renderer();
    }

    fn handle_surface_changed(&mut self, width: i32, height: i32) {
        // Window resized or rotated
        self.bounds = Bounds { ... };
        self.resize_renderer();
    }

    fn handle_surface_destroyed(&mut self) {
        // ANativeWindow will be destroyed
        // CRITICAL: Release Vulkan resources!
        self.renderer.cleanup();
        self.native_window = None;
    }
}
```

**Key challenge:** The ANativeWindow can be destroyed and recreated during app lifecycle (screen rotation, app backgrounding).

**Sources:**

- [winit Android raw_window_handle usage requirements](https://github.com/rust-windowing/winit/issues/1588)
- [Android interactions in raw-window-handle](https://github.com/rust-windowing/raw-window-handle/issues/54)

### 2. Activity and Application (NEW)

**File:** `src/platform/android/activity.rs` (~500 lines)

**Using android-activity crate:**

```rust
use android_activity::{
    AndroidApp, MainEvent, InputStatus, PollEvent,
    input::KeyEvent, input::MotionEvent,
};

pub struct AndroidPlatform {
    app: AndroidApp,
    common: AndroidCommon,
    window: Option<Rc<RefCell<AndroidWindow>>>,
}

impl AndroidPlatform {
    pub fn new(app: AndroidApp) -> Self {
        // Initialize platform
    }

    pub fn run(&self) {
        loop {
            self.app.poll_events(|event| {
                match event {
                    PollEvent::Main(main_event) => {
                        self.handle_main_event(main_event)
                    }
                    PollEvent::Wake => {
                        self.handle_wake()
                    }
                    _ => {}
                }
            });
        }
    }

    fn handle_main_event(&mut self, event: MainEvent) {
        match event {
            MainEvent::InitWindow { .. } => {
                // Surface available
            }
            MainEvent::TerminateWindow { .. } => {
                // Surface destroyed
            }
            MainEvent::WindowResized { width, height } => {
                // Handle resize
            }
            MainEvent::InputAvailable => {
                // Process input
            }
            MainEvent::Pause => {
                // App going to background
            }
            MainEvent::Resume => {
                // App coming to foreground
            }
            _ => {}
        }
    }
}
```

**Library:** [rust-mobile/ndk](https://github.com/rust-mobile/ndk) with android-activity 0.6.0+

### 3. Input Handling (PARTIAL REUSE)

**File:** `src/platform/android/input.rs` (~800 lines)

**Touch Events:**

```rust
fn handle_motion_event(&mut self, event: &MotionEvent) -> InputStatus {
    match event.action() {
        MotionAction::Down => {
            let position = Point {
                x: px(event.x()),
                y: px(event.y()),
            };
            self.dispatch_mouse_event(MouseDown {
                button: MouseButton::Left,
                position,
                // ...
            })
        }
        MotionAction::Move => {
            // Track multi-touch
        }
        MotionAction::Up => {
            // Mouse up
        }
        _ => InputStatus::Unhandled
    }
}
```

**Keyboard Events:**

```rust
fn handle_key_event(&mut self, event: &KeyEvent) -> InputStatus {
    // Map Android KeyCode to GPUI Keystroke
    let keystroke = self.map_key_event(event);

    self.dispatch_keystroke_event(KeystrokeEvent {
        keystroke,
        is_held: event.repeat_count() > 0,
    })
}
```

**Soft Keyboard:**

```rust
impl AndroidWindow {
    pub fn show_soft_keyboard(&self) {
        // JNI call to InputMethodManager
        self.app.show_soft_input();
    }

    pub fn hide_soft_keyboard(&self) {
        self.app.hide_soft_input();
    }
}
```

### 4. System Integration (NEW)

**File:** `src/platform/android/integration.rs` (~400 lines)

#### Clipboard

```rust
use ndk::clipboard::ClipboardManager;

impl AndroidPlatform {
    fn write_to_clipboard(&self, item: ClipboardItem) {
        let clipboard = self.app.clipboard_manager();
        match item {
            ClipboardItem::Text(text) => {
                clipboard.set_primary_clip(text);
            }
            // Image support via ContentProvider
            _ => {}
        }
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        let clipboard = self.app.clipboard_manager();
        clipboard.get_primary_clip()
            .and_then(|clip| clip.get_text())
            .map(ClipboardItem::Text)
    }
}
```

#### File Picker (Intent-based)

```rust
impl AndroidPlatform {
    fn open_file_picker(&self, options: PathPromptOptions) {
        // Launch Intent for ACTION_OPEN_DOCUMENT
        let intent = Intent::new()
            .action("android.intent.action.OPEN_DOCUMENT")
            .category("android.intent.category.OPENABLE");

        // Result handled via Activity callback
        self.app.start_activity_for_result(intent);
    }
}
```

**Challenge:** Android uses asynchronous Intent system rather than synchronous dialogs.

#### URL Opening

```rust
fn open_url(&self, url: &str) {
    let intent = Intent::new()
        .action("android.intent.action.VIEW")
        .data(url);

    self.app.start_activity(intent);
}
```

### 5. Display Information (PARTIAL REUSE)

**File:** `src/platform/android/display.rs` (~200 lines)

```rust
use ndk::configuration::Configuration;

pub struct AndroidDisplay {
    display_id: DisplayId,
    bounds: Bounds<Pixels>,
    scale_factor: f32,
    refresh_rate: f32,
}

impl AndroidDisplay {
    fn from_window_manager(app: &AndroidApp) -> Vec<Self> {
        // Query WindowManager via JNI
        // Get Display metrics
        let config = app.config();

        vec![AndroidDisplay {
            display_id: DisplayId(0),
            bounds: Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(config.screen_width_dp() as f32),
                    height: px(config.screen_height_dp() as f32),
                },
            },
            scale_factor: config.density() / 160.0, // Android density to scale
            refresh_rate: 60.0, // Query from Display.getRefreshRate()
        }]
    }
}
```

**Android specifics:**

- Density-independent pixels (dp) vs physical pixels
- Scale factor from density (ldpi=0.75, mdpi=1.0, hdpi=1.5, xhdpi=2.0, xxhdpi=3.0)
- Notch/cutout handling

---

## Architecture Overview

### File Structure

```
src/platform/android/
├── mod.rs              # Module exports
├── platform.rs         # AndroidPlatform implementation (~600 lines)
├── window.rs           # AndroidWindow implementation (~1,500 lines)
├── activity.rs         # Activity lifecycle management (~500 lines)
├── input.rs            # Input event handling (~800 lines)
├── display.rs          # Display information (~200 lines)
├── integration.rs      # Clipboard, intents, system (~400 lines)
├── keyboard.rs         # Keyboard mapping (~300 lines)
└── dispatcher.rs       # Android event loop integration (~400 lines)

Total: ~4,700 new lines (compare to Linux: ~12,937 lines)
```

### Platform Implementation Pattern

Following Linux model:

```rust
// src/platform/android/platform.rs

pub trait AndroidClient {
    fn with_common<R>(&self, f: impl FnOnce(&mut AndroidCommon) -> R) -> R;
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>>;
    fn open_window(&self, handle: AnyWindowHandle, options: WindowParams)
        -> anyhow::Result<Box<dyn PlatformWindow>>;
    fn run(&self);
    // ... Android-specific methods
}

pub(crate) struct AndroidCommon {
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) text_system: Arc<dyn PlatformTextSystem>,  // CosmicText!
    pub(crate) appearance: WindowAppearance,
    pub(crate) callbacks: PlatformHandlers,
}

impl<P: AndroidClient + 'static> Platform for P {
    fn background_executor(&self) -> BackgroundExecutor {
        self.with_common(|common| common.background_executor.clone())
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.with_common(|common| common.text_system.clone())
    }

    fn open_window(&self, handle: AnyWindowHandle, options: WindowParams)
        -> Box<dyn PlatformWindow>
    {
        self.open_window(handle, options).unwrap()
    }

    // ... implement all Platform methods
}

pub struct AndroidPlatform {
    app: AndroidApp,
    common: AndroidCommon,
    blade_context: BladeContext,  // Reused from Linux!
    window: Option<Rc<RefCell<AndroidWindow>>>,
}
```

### Integration with Blade Renderer

**No changes needed to Blade code!**

```rust
// src/platform/android/window.rs

impl AndroidWindow {
    fn new(
        app: AndroidApp,
        native_window: NativeWindow,
        blade_context: &BladeContext,
        handle: AnyWindowHandle,
    ) -> anyhow::Result<Self> {
        // Create raw window handle
        let raw_window = RawWindow {
            window: native_window.ptr().as_ptr() as *mut c_void,
        };

        // Initialize Blade renderer (same as Linux!)
        let renderer = BladeRenderer::new(
            blade_context,
            &raw_window,  // Implements HasWindowHandle
            size,
        )?;

        Ok(Self {
            native_window: Some(native_window),
            renderer,
            // ...
        })
    }
}

impl PlatformWindow for AndroidWindow {
    fn draw(&mut self, scene: &Scene) {
        // Reuse Blade renderer!
        self.renderer.draw(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.renderer.sprite_atlas()
    }

    // ... implement other PlatformWindow methods
}
```

---

## Implementation Phases

### Phase 1: MVP - Basic Rendering (2-3 months)

**Goal:** Display GPUI UI on Android, handle basic input

**Tasks:**

#### 1.1 Project Setup (1 week)

- [ ] Add Android targets to Cargo.toml

  ```toml
  [target.'cfg(target_os = "android")'.dependencies]
  android-activity = "0.6"
  ndk = "0.9"
  ndk-sys = "0.6"
  raw-window-handle = "0.6"
  ```

- [ ] Create `src/platform/android/` directory structure
- [ ] Add feature flag: `android = ["blade-graphics", "cosmic-text", "android-activity", "ndk"]`
- [ ] Setup NDK integration in build.rs
- [ ] Configure Rust Android toolchain

#### 1.2 Platform Trait Implementation (2 weeks)

- [ ] Implement `AndroidCommon` struct (reusing Linux pattern)
- [ ] Implement `AndroidClient` trait
- [ ] Implement `Platform` trait for AndroidPlatform
- [ ] Setup executors (BackgroundExecutor, ForegroundExecutor)
- [ ] Initialize CosmicText system (copy from Linux)

**Priority:** High - Foundation for everything

#### 1.3 Window Management (3 weeks)

- [ ] Create `AndroidWindow` struct
- [ ] Implement surface lifecycle callbacks
  - `surface_created`
  - `surface_changed`
  - `surface_destroyed`
- [ ] Integrate raw-window-handle for ANativeWindow
- [ ] Initialize Blade renderer with Android surface
- [ ] Implement `PlatformWindow` trait
- [ ] Handle window resize/rotation

**Priority:** Critical - Required for rendering

#### 1.4 Basic Input (2 weeks)

- [ ] Implement touch event handling
  - Map MotionEvent to MouseDown/MouseMove/MouseUp
  - Single touch to mouse events
- [ ] Basic key event handling
  - Map Android KeyCode to GPUI Keystroke
  - Handle back button
- [ ] Test with simple UI (button, text)

**Priority:** High - Required for interaction

#### 1.5 Hello World Example (1 week)

- [ ] Create `examples/android_hello_world/`
- [ ] Setup Android project structure
- [ ] Build APK
- [ ] Test on emulator
- [ ] Test on physical device

**Milestone:** Can display "Hello World" and respond to touch

### Phase 2: Complete Features (1-2 months)

**Goal:** Full-featured Android platform

#### 2.1 Advanced Input (2 weeks)

- [ ] Multi-touch support
- [ ] Gesture recognition (pinch, zoom, swipe)
- [ ] Soft keyboard integration
  - Show/hide keyboard
  - IME composition
  - Handle keyboard resize
- [ ] Hardware keyboard support (Bluetooth, USB)
- [ ] Gamepad/controller support

#### 2.2 System Integration (2 weeks)

- [ ] Clipboard implementation (text)
- [ ] Clipboard implementation (images via ContentProvider)
- [ ] URL opening via Intent
- [ ] File picker via Intent
  - Handle async result
  - Permissions (READ_EXTERNAL_STORAGE)
- [ ] Share sheet integration

#### 2.3 Display and Appearance (1 week)

- [ ] Display metrics and scaling
- [ ] Notch/cutout detection and handling
- [ ] Dark mode detection
- [ ] System theme colors (Material You)
- [ ] Edge-to-edge rendering (Android 10+)

#### 2.4 Lifecycle and Background (1 week)

- [ ] Handle app pause/resume
- [ ] Save/restore state
- [ ] Background rendering suspension
- [ ] Low memory warnings
- [ ] Battery optimization

#### 2.5 Permissions (1 week)

- [ ] Runtime permission requests
- [ ] Storage permissions
- [ ] Camera/microphone permissions (for future)
- [ ] Permission result handling

**Milestone:** Feature-complete Android platform

### Phase 3: Polish and Optimization (1 month)

#### 3.1 Performance Optimization (2 weeks)

- [ ] Profile rendering performance
- [ ] Optimize texture atlas for mobile
- [ ] Reduce memory footprint
- [ ] Battery usage optimization
- [ ] Reduce APK size
- [ ] Startup time optimization

#### 3.2 Testing (1 week)

- [ ] Unit tests for Android platform
- [ ] Integration tests
- [ ] Test on various devices
  - Different screen sizes
  - Different Android versions (API 24-34)
  - Different manufacturers (Samsung, Pixel, OnePlus)
- [ ] Automated UI tests

#### 3.3 Documentation (1 week)

- [ ] Android platform API documentation
- [ ] Setup guide for Android development
- [ ] APK building instructions
- [ ] Play Store deployment guide
- [ ] Example applications
- [ ] Migration guide from desktop

#### 3.4 Examples (1 week)

- [ ] `android_hello_world` - Basic app
- [ ] `android_list` - Scrolling list
- [ ] `android_input` - Keyboard, touch, gestures
- [ ] `android_image_gallery` - Image loading
- [ ] `android_counter` - State management

**Milestone:** Production-ready Android support

---

## Technical Challenges and Solutions

### Challenge 1: ANativeWindow Lifecycle

**Problem:** ANativeWindow can be destroyed and recreated during app lifecycle (rotation, background).

**Solution:**

```rust
impl AndroidWindow {
    fn handle_surface_destroyed(&mut self) {
        // 1. Wait for outstanding frames
        self.renderer.wait_for_frame_completion();

        // 2. Release Vulkan surface
        self.renderer.destroy_surface();

        // 3. Clear window reference
        self.native_window = None;

        // Keep other state intact for recreation!
    }

    fn handle_surface_created(&mut self, window: NativeWindow) {
        // 1. Store new window
        self.native_window = Some(window);

        // 2. Recreate Vulkan surface
        self.renderer.recreate_surface(&window);

        // 3. Restore rendering state
        self.renderer.restore_state();
    }
}
```

**Testing:** Repeatedly rotate device to stress-test lifecycle.

### Challenge 2: Soft Keyboard Resize

**Problem:** Soft keyboard appearance resizes window, causing layout reflow.

**Solution:**

```rust
impl AndroidWindow {
    fn handle_content_rect_changed(&mut self, rect: Bounds<Pixels>) {
        // 1. Calculate keyboard height
        let keyboard_height = self.full_bounds.size.height - rect.size.height;

        // 2. Adjust viewport (not full window)
        self.content_bounds = rect;

        // 3. Notify GPUI for layout
        if let Some(callback) = &mut self.callbacks.resize {
            callback(rect.size, self.scale);
        }

        // 4. Scroll to keep focused element visible
        self.scroll_to_show_focus();
    }
}
```

**Alternative:** Use `adjustNothing` and handle manually with insets.

### Challenge 3: Intent-Based File Picker (Async)

**Problem:** Linux file picker is synchronous, Android is async via Intent.

**Solution:**

```rust
impl AndroidPlatform {
    fn open_file_picker(
        &self,
        options: PathPromptOptions,
        callback: Box<dyn FnOnce(Option<Vec<PathBuf>>) + 'static>,
    ) {
        // 1. Store callback with request ID
        let request_id = self.next_request_id();
        self.pending_intent_callbacks.insert(request_id, callback);

        // 2. Launch Intent
        let intent = Intent::new()
            .action("android.intent.action.OPEN_DOCUMENT")
            .add_category("android.intent.category.OPENABLE")
            .set_type(options.file_types.join(","));

        self.app.start_activity_for_result(intent, request_id);
    }

    fn handle_activity_result(
        &mut self,
        request_id: i32,
        result_code: i32,
        data: Option<Intent>,
    ) {
        // 3. Retrieve callback
        if let Some(callback) = self.pending_intent_callbacks.remove(&request_id) {
            // 4. Extract file URI
            let paths = data
                .and_then(|intent| intent.data())
                .map(|uri| vec![uri_to_path(uri)]);

            // 5. Invoke callback
            callback(paths);
        }
    }
}
```

**API Change:** File picker methods must accept callback (breaking change).

### Challenge 4: Permission Requests

**Problem:** Requires async permission requests before certain operations.

**Solution:**

```rust
impl AndroidPlatform {
    fn request_storage_permission(&self) -> Task<bool> {
        self.foreground_executor().spawn(async move {
            let request_id = self.next_request_id();
            let (tx, rx) = oneshot::channel();

            self.pending_permission_callbacks.insert(request_id, tx);

            self.app.request_permissions(
                &["android.permission.READ_EXTERNAL_STORAGE"],
                request_id,
            );

            rx.await.unwrap_or(false)
        })
    }

    async fn open_file_picker_with_permission(
        &self,
        options: PathPromptOptions,
    ) -> Option<Vec<PathBuf>> {
        // Check and request permission first
        if !self.has_storage_permission() {
            let granted = self.request_storage_permission().await;
            if !granted {
                return None;
            }
        }

        // Then open picker
        self.open_file_picker_async(options).await
    }
}
```

**API Impact:** Operations requiring permissions must be async.

### Challenge 5: Text Input and IME

**Problem:** Soft keyboard IME composition needs special handling.

**Solution:**

```rust
impl AndroidWindow {
    fn handle_text_input(&mut self, event: TextInputEvent) {
        match event {
            TextInputEvent::Composing { text, cursor } => {
                // Show composition preview
                self.set_composition_range(text, cursor);
            }
            TextInputEvent::Commit { text } => {
                // Finalize input
                self.insert_text(text);
                self.clear_composition();
            }
            TextInputEvent::DeleteBackward { count } => {
                self.delete_backward(count);
            }
        }
    }
}
```

**Testing:** Test with various keyboards (Gboard, SwiftKey) and languages (Chinese, Japanese, Korean).

### Challenge 6: Screen Density Variations

**Problem:** Android devices have wildly varying densities (120dpi to 640dpi).

**Solution:**

```rust
impl AndroidDisplay {
    fn calculate_scale_factor(density_dpi: i32) -> f32 {
        // Android baseline is 160 DPI (mdpi)
        density_dpi as f32 / 160.0
    }

    fn logical_to_physical(&self, logical: Pixels) -> DevicePixels {
        DevicePixels((logical.0 * self.scale_factor).round() as i32)
    }
}
```

**Testing:** Test on devices from mdpi (1x) to xxxhdpi (4x).

---

## Dependencies and Prerequisites

### Rust Dependencies

Add to `Cargo.toml`:

```toml
[target.'cfg(target_os = "android")'.dependencies]
# Android NDK bindings
android-activity = { version = "0.6", features = ["native-activity"] }
ndk = "0.9"
ndk-context = "0.1"
ndk-sys = "0.6"

# Already used for Linux (no changes needed!)
blade-graphics = { workspace = true }
blade-macros = { workspace = true }
blade-util = { workspace = true }
cosmic-text = "0.14.0"
swash = "0.2.6"

# Platform abstraction
raw-window-handle = "0.6"

# Async/concurrency (already present)
futures = { workspace = true }
smol = { workspace = true }
```

### System Requirements

**Development:**

- Android SDK Platform 24+ (Android 7.0+)
- Android NDK r25+
- Rust 1.75+ with Android targets:
  ```bash
  rustup target add aarch64-linux-android    # ARM64 (primary)
  rustup target add armv7-linux-androideabi  # ARM32 (legacy)
  rustup target add x86_64-linux-android     # x86_64 (emulator)
  rustup target add i686-linux-android       # x86 (legacy emulator)
  ```

**Runtime:**

- Android 7.0+ (API 24+) for Vulkan support
- Vulkan 1.0+ capable device (~99% of Android 7.0+ devices)
- Recommended: Android 10+ (API 29+) for best experience

**Verification:**

According to [Android documentation](https://developer.android.com/ndk/guides/graphics/getting-started), Vulkan support is available on devices running Android 7.0 (API 24) and higher.

---

## Testing Strategy

### Unit Tests

```rust
#[cfg(target_os = "android")]
#[test]
fn test_android_window_lifecycle() {
    // Test surface creation/destruction
}

#[cfg(target_os = "android")]
#[test]
fn test_input_mapping() {
    // Test KeyCode to Keystroke mapping
}
```

### Integration Tests

```rust
#[gpui::test]
fn test_android_rendering(cx: &mut TestAppContext) {
    // Use GPUI test infrastructure
    // Mock ANativeWindow
}
```

### Device Testing Matrix

| Device Type | Android Version | Screen     | Density       | Priority |
| ----------- | --------------- | ---------- | ------------- | -------- |
| Pixel 7     | 14 (API 34)     | 6.3" FHD+  | xxhdpi (3x)   | High     |
| Pixel 4a    | 13 (API 33)     | 5.8" FHD   | xxhdpi (2.5x) | High     |
| Samsung S23 | 14 (API 34)     | 6.1" FHD+  | xxxhdpi (4x)  | High     |
| OnePlus 9   | 13 (API 33)     | 6.55" FHD+ | xhdpi (2.5x)  | Medium   |
| Emulator    | 10 (API 29)     | Various    | Various       | High     |
| Emulator    | 7 (API 24)      | Various    | Various       | Medium   |

### Automated Testing

**Use cargo-ndk for building:**

```bash
cargo install cargo-ndk

# Build for ARM64
cargo ndk --target aarch64-linux-android --platform 29 build

# Run tests on emulator
cargo ndk --target aarch64-linux-android --platform 29 test
```

**CI/CD:**

```yaml
# .github/workflows/android.yml
name: Android CI

on: [push, pull_request]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Setup Rust
        run: rustup target add aarch64-linux-android

      - name: Setup Android SDK
        uses: android-actions/setup-android@v2

      - name: Install cargo-ndk
        run: cargo install cargo-ndk

      - name: Build
        run: cargo ndk --target aarch64-linux-android --platform 29 build

      - name: Test
        run: cargo ndk --target aarch64-linux-android --platform 29 test
```

---

## Performance Considerations

### Rendering Performance

**Target:** 60 FPS on mid-range devices (Snapdragon 700 series)

**Optimizations:**

1. **Reduce overdraw:**

   ```rust
   // Use scene culling for off-screen elements
   scene.cull_to_viewport(viewport_bounds);
   ```

2. **Texture atlas optimization:**

   ```rust
   // Smaller atlas for mobile (2048x2048 instead of 4096x4096)
   const MOBILE_ATLAS_SIZE: u32 = 2048;
   ```

3. **Frame pacing:**
   ```rust
   // Use choreographer for vsync
   self.app.set_frame_callback(|duration| {
       // Render at display refresh rate
   });
   ```

### Memory Optimization

**Target:** < 200MB for basic app

**Strategies:**

1. **Lazy font loading:**

   ```rust
   // Load fonts on-demand, not all at startup
   text_system.load_font_lazy(&family);
   ```

2. **Texture atlas eviction:**

   ```rust
   // Aggressive LRU eviction on mobile
   atlas.set_max_memory(100 * 1024 * 1024); // 100MB
   ```

3. **Handle low memory warnings:**
   ```rust
   fn handle_low_memory(&mut self) {
       self.atlas.clear();
       self.font_cache.clear();
       // Force garbage collection
   }
   ```

### Battery Optimization

**Strategies:**

1. **Reduce render when invisible:**

   ```rust
   fn handle_pause(&mut self) {
       self.suspend_rendering();
   }
   ```

2. **Adaptive frame rate:**
   ```rust
   // 60 FPS when animating, 30 FPS when static
   if self.is_animating() {
       self.request_frame(16); // ~60 FPS
   } else {
       self.request_frame(33); // ~30 FPS
   }
   ```

### Startup Time

**Target:** < 1 second cold start

**Optimizations:**

1. **Lazy initialization:**

   ```rust
   // Don't load all fonts at startup
   // Don't pre-compile all shaders
   ```

2. **Parallel initialization:**
   ```rust
   // Initialize subsystems in parallel
   let text_system = spawn(|| CosmicTextSystem::new());
   let blade_context = spawn(|| BladeContext::new());
   ```

---

## Open Questions

### 1. Multi-Window Support

**Question:** Should we support Android multi-window mode (split-screen)?

**Impact:** Medium - Requires handling multiple surfaces, coordinate systems

**Decision needed:** Phase 2 or Phase 3?

**Recommendation:** Phase 3 (nice-to-have, not critical for MVP)

### 2. Foldable Device Support

**Question:** Support for foldable displays (Samsung Fold, Pixel Fold)?

**Challenges:**

- Screen configuration changes mid-session
- Different aspect ratios
- Hinge detection

**Recommendation:** Phase 3+ (small market share, complex)

### 3. Widget Support

**Question:** Should we support Android App Widgets (home screen widgets)?

**Impact:** High - Requires separate process, limited UI

**Recommendation:** Out of scope for initial release

### 4. Build System

**Question:** Use cargo-ndk, gradle-rust plugin, or custom build system?

**Options:**

- **cargo-ndk:** Simple, developer-friendly, limited gradle integration
- **gradle-rust:** Better gradle integration, more complex
- **Custom:** Maximum control, high maintenance

**Recommendation:** Start with cargo-ndk, evaluate gradle-rust in Phase 2

### 5. Minimum API Level

**Question:** Target API 24 (Vulkan support) or higher?

**API 24 (Android 7.0, 2016):**

- Pro: Maximum device coverage (~99% of active devices)
- Con: Missing modern features

**API 29 (Android 10, 2019):**

- Pro: Modern features (edge-to-edge, dark mode, etc.)
- Con: Excludes ~10% of devices

**Recommendation:** API 24 for compatibility, feature-detect API 29+ features

---

## Resources and References

### Documentation

- [Rust Android NDK Bindings](https://github.com/rust-mobile/ndk)
- [android-activity crate](https://crates.io/crates/android-activity)
- [raw-window-handle Android support](https://docs.rs/raw-window-handle/)
- [blade-graphics](https://crates.io/crates/blade-graphics)
- [cosmic-text](https://crates.io/crates/cosmic-text)

### Android Platform

- [Android NDK Documentation](https://developer.android.com/ndk)
- [Vulkan on Android](https://developer.android.com/ndk/guides/graphics/getting-started)
- [Android App Architecture](https://developer.android.com/guide/components/fundamentals)
- [Android Input System](https://developer.android.com/develop/ui/views/touch-and-input/input-events)

### Examples and Prior Art

- [Iced Android Example (2025)](https://www.webpronews.com/rusts-iced-gui-integrates-with-android-for-native-apps/) - Recent success story of Rust GUI on Android
- [winit Android Support](https://github.com/rust-windowing/winit) - Reference implementation
- [rust-mobile examples](https://github.com/rust-mobile/ndk/tree/main/ndk-examples)

### Web Search Sources

The following sources were used for research:

**Rust + Android:**

- [Rust Android NDK Update](https://blog.rust-lang.org/2023/01/09/android-ndk-update-r25.html)
- [rust-mobile/ndk Repository](https://github.com/rust-mobile/ndk)
- [Android Rust Introduction](https://source.android.com/docs/setup/build/rust/building-rust-modules/overview)

**Vulkan Support:**

- [Get started with Vulkan on Android](https://developer.android.com/ndk/guides/graphics/getting-started)

**Window Handling:**

- [winit Android Support](https://github.com/rust-windowing/winit)
- [Winit Rust Guide [2025]](https://generalistprogrammer.com/tutorials/winit-rust-crate-guide)
- [Android raw_window_handle Issues](https://github.com/rust-windowing/winit/issues/1588)

**Recent Success:**

- [Rust's Iced GUI Integrates with Android](https://www.webpronews.com/rusts-iced-gui-integrates-with-android-for-native-apps/)

---

## Conclusion

**Android support for GPUI is highly feasible** with estimated 70% code reuse from the Linux implementation. The key advantages:

✅ **Blade renderer (Vulkan) works out-of-box on Android**
✅ **CosmicText (pure Rust) requires no platform changes**
✅ **Core GPUI framework is platform-agnostic**
✅ **Recent success stories (iced-android, 2025) validate approach**
✅ **Strong tooling support (rust-mobile/ndk, android-activity)**

**Primary effort** will be in:

- Window lifecycle management (~1,500 lines)
- Input handling and soft keyboard (~800 lines)
- System integration (clipboard, intents) (~800 lines)
- Android-specific platform glue (~1,200 lines)

**Total new code:** ~4,700 lines (vs. Linux 12,937 lines = **64% less code**)

**Recommended Approach:**

1. Start with Phase 1 MVP (2-3 months)
2. Validate on real devices early and often
3. Iterate based on performance and user feedback
4. Complete Phase 2 features (1-2 months)
5. Polish in Phase 3 (1 month)

**Total estimated time:** 4-6 months to production-ready Android support.

---

**Plan Status:** READY FOR REVIEW
**Next Steps:**

1. Team review and feedback
2. Resource allocation
3. Begin Phase 1 implementation
4. Set up Android development environment
5. Create tracking issues for each task

**Last Updated:** 2026-01-07
