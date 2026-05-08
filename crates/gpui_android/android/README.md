# gpui_android Android library sources

Kotlin classes that ship with the `gpui_android` crate. Mirrors how `gpui_ios`
registers `GPUIMetalView` at runtime via `objc::declare::ClassDecl` — Android
has no equivalent runtime registration for `View` subclasses, so the framework
ships a `SurfaceView` and a runtime controller as Kotlin source.

## Integration

Add this directory to your downstream Android Gradle module. In
`android/build.gradle` (or the consuming module's `build.gradle.kts`):

```groovy
sourceSets {
    main {
        kotlin.srcDirs = [
            'app/src/main/kotlin',
            // gpui_android framework — adjust the relative path to taste
            '../vendor/zed/crates/gpui_android/android/src/main/kotlin',
        ]
    }
}
```

Required AndroidX dependency for `WindowInsetsCompat`:

```groovy
implementation 'androidx.core:core-ktx:1.13.1'
```

## What lives here

- `GpuiSurfaceView.kt` — `SurfaceView` + `SurfaceHolder.Callback`. Owns surface
  lifecycle, touch / fling / IME forwarding, and IME inset reporting. Calls
  into the `gpui_android` Rust crate via JNI symbols
  `Java_dev_zed_gpui_GpuiSurfaceView_*`.
- `GpuiRuntimeController.kt` — Mirrors `GPUIRuntimeController.swift`. Bridges
  the host `Activity` lifecycle (`onResume` / `onPause` / `onStop` /
  `onDestroy`) to the framework, wires up the `Choreographer` frame loop, and
  installs a `GpuiSurfaceView` into a parent layout.

The downstream app's `MainActivity` should be a thin shell that constructs the
controller, calls its lifecycle methods, and triggers the Rust entry point that
runs `gpui_android::run(...)`.
