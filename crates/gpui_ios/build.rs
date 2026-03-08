#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]

fn main() {
    // Build scripts run on the host, not the target. Use CARGO_CFG_TARGET_OS
    // to detect the cross-compilation target instead of #[cfg(target_os)].
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "ios" {
        ios_build::run();
    }
}

mod ios_build {
    use std::{
        env,
        path::{Path, PathBuf},
    };

    use cbindgen::Config;

    fn is_simulator() -> bool {
        env::var("TARGET")
            .unwrap_or_default()
            .contains("-sim")
    }

    fn metal_sdk() -> &'static str {
        if is_simulator() { "iphonesimulator" } else { "iphoneos" }
    }

    pub fn run() {
        generate_dispatch_bindings();

        let header_path = generate_shader_bindings();

        compile_metal_shaders(&header_path);
    }

    fn generate_dispatch_bindings() {
        // libdispatch is part of libSystem on iOS (not a separate framework)
        println!("cargo:rustc-link-lib=dylib=System");

        let clang_target = if is_simulator() {
            "--target=arm64-apple-ios-simulator"
        } else {
            "--target=arm64-apple-ios"
        };

        let bindings = bindgen::Builder::default()
            .header("src/dispatch.h")
            .allowlist_var("_dispatch_main_q")
            .allowlist_var("_dispatch_source_type_data_add")
            .allowlist_var("DISPATCH_QUEUE_PRIORITY_HIGH")
            .allowlist_var("DISPATCH_QUEUE_PRIORITY_DEFAULT")
            .allowlist_var("DISPATCH_QUEUE_PRIORITY_LOW")
            .allowlist_var("DISPATCH_TIME_NOW")
            .allowlist_function("dispatch_get_global_queue")
            .allowlist_function("dispatch_async_f")
            .allowlist_function("dispatch_after_f")
            .allowlist_function("dispatch_time")
            .allowlist_function("dispatch_source_merge_data")
            .allowlist_function("dispatch_source_create")
            .allowlist_function("dispatch_source_set_event_handler_f")
            .allowlist_function("dispatch_resume")
            .allowlist_function("dispatch_suspend")
            .allowlist_function("dispatch_source_cancel")
            .allowlist_function("dispatch_set_context")
            .clang_arg(clang_target)
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            .layout_tests(false)
            .generate()
            .expect("unable to generate bindings");

        let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
        bindings
            .write_to_file(out_path.join("dispatch_sys.rs"))
            .expect("couldn't write dispatch bindings");
    }

    fn generate_shader_bindings() -> PathBuf {
        let output_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("scene.h");

        let gpui_dir = find_gpui_crate_dir();

        let mut config = Config {
            include_guard: Some("SCENE_H".into()),
            language: cbindgen::Language::C,
            no_includes: true,
            ..Default::default()
        };
        config.export.include.extend([
            "Bounds".into(),
            "Corners".into(),
            "Edges".into(),
            "Size".into(),
            "Pixels".into(),
            "PointF".into(),
            "Hsla".into(),
            "ContentMask".into(),
            "Uniforms".into(),
            "AtlasTile".into(),
            "PathRasterizationInputIndex".into(),
            "PathVertex_ScaledPixels".into(),
            "PathRasterizationVertex".into(),
            "ShadowInputIndex".into(),
            "Shadow".into(),
            "QuadInputIndex".into(),
            "Underline".into(),
            "UnderlineInputIndex".into(),
            "Quad".into(),
            "BorderStyle".into(),
            "SpriteInputIndex".into(),
            "MonochromeSprite".into(),
            "PolychromeSprite".into(),
            "PathSprite".into(),
            "SurfaceInputIndex".into(),
            "SurfaceBounds".into(),
            "TransformationMatrix".into(),
        ]);
        config.no_includes = true;
        config.enumeration.prefix_with_name = true;

        let mut builder = cbindgen::Builder::new();

        let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

        let gpui_src_paths = [
            gpui_dir.join("src/scene.rs"),
            gpui_dir.join("src/geometry.rs"),
            gpui_dir.join("src/color.rs"),
            gpui_dir.join("src/window.rs"),
            gpui_dir.join("src/platform.rs"),
        ];

        let local_src_paths = [crate_dir.join("src/metal_renderer.rs")];

        for src_path in gpui_src_paths.iter().chain(local_src_paths.iter()) {
            println!("cargo:rerun-if-changed={}", src_path.display());
            builder = builder.with_src(src_path);
        }

        builder
            .with_config(config)
            .generate()
            .expect("Unable to generate bindings")
            .write_to_file(&output_path);

        output_path
    }

    fn find_gpui_crate_dir() -> PathBuf {
        gpui::GPUI_MANIFEST_DIR.into()
    }

    fn compile_metal_shaders(header_path: &Path) {
        use std::process::{self, Command};

        let sdk = metal_sdk();
        let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let shader_path = crate_dir.join("src/shaders.metal");
        let air_output_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("shaders.air");
        let metallib_output_path =
            PathBuf::from(env::var("OUT_DIR").unwrap()).join("shaders.metallib");
        println!("cargo:rerun-if-changed={}", shader_path.display());

        let mut cmd = Command::new("xcrun");
        cmd.args([
            "-sdk", sdk,
            "metal",
            "-gline-tables-only",
            "-mios-version-min=16.0",
            "-MO",
            "-c",
        ]);
        if is_simulator() {
            cmd.arg("-target").arg("air64-apple-ios18.1-simulator");
        }
        let output = cmd
            .arg(&shader_path)
            .arg("-include")
            .arg(header_path)
            .arg("-o")
            .arg(&air_output_path)
            .output()
            .unwrap();

        if !output.status.success() {
            println!(
                "cargo::error=metal shader compilation failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            process::exit(1);
        }

        let output = Command::new("xcrun")
            .args(["-sdk", sdk, "metallib"])
            .arg(air_output_path)
            .arg("-o")
            .arg(metallib_output_path)
            .output()
            .unwrap();

        if !output.status.success() {
            println!(
                "cargo::error=metallib compilation failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            process::exit(1);
        }
    }
}
