#[cfg(target_os = "macos")]
fn compile_apple_vision_ocr() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("../ocr/vision_ocr.swift");
    let res_dir = manifest_dir.join("resources");
    let dst = res_dir.join("vision_ocr");

    println!("cargo:rerun-if-changed={}", src.display());

    if !src.exists() {
        panic!("Missing Apple Vision OCR source at {}", src.display());
    }
    fs::create_dir_all(&res_dir).expect("Failed to create src-tauri/resources");

    let needs_build = match (fs::metadata(&src), fs::metadata(&dst)) {
        (Ok(sm), Ok(dm)) => match (sm.modified(), dm.modified()) {
            (Ok(st), Ok(dt)) => st > dt,
            _ => true,
        },
        _ => true,
    };

    if needs_build {
        let mut cmd = Command::new("swiftc");
        cmd.args([
            "-O",
            "-framework",
            "Vision",
            "-framework",
            "ImageIO",
            "-framework",
            "CoreGraphics",
        ]);
        if let Ok(arch) = std::env::var("CARGO_CFG_TARGET_ARCH") {
            if arch == "aarch64" {
                cmd.args(["-target", "arm64-apple-macosx14.0"]);
            }
        }
        cmd.arg(&src).arg("-o").arg(&dst);
        let status = cmd.status().expect("Failed to invoke swiftc");
        if !status.success() {
            panic!("swiftc failed compiling vision_ocr.swift");
        }
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    compile_apple_vision_ocr();

    tauri_build::build();
}
