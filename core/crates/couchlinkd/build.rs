// Puts the version information (company, product, version) into couchlinkd.exe. On macOS it also builds the input
// library a game copy loads (hook/macos/couchlink_inject.m) and the probe program its tests start.
fn main() {
    println!("cargo:rerun-if-changed=couchlinkd.rc");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "windows" {
        embed_resource::compile("couchlinkd.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
    if os == "macos" {
        build_macos();
    }
}

fn build_macos() {
    use std::path::PathBuf;
    use std::process::Command;
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../../hook/macos");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "x86_64",
    };
    let frameworks = ["Foundation", "AppKit", "GameController", "IOKit", "CoreGraphics"];
    let mut parts: Vec<(&str, &str, Vec<&str>)> = vec![
        ("couchlink_inject.m", "libcouchlink_inject.dylib", vec!["-dynamiclib", "-install_name", "@rpath/libcouchlink_inject.dylib"]),
        ("inputprobe.m", "inputprobe", vec![]),
    ];
    for (file, name, extra) in parts.drain(..) {
        println!("cargo:rerun-if-changed={}", src.join(file).display());
        let mut cmd = Command::new("clang");
        cmd.args(["-fobjc-arc", "-O2", "-Wall", "-mmacosx-version-min=11.0", "-arch", arch]).args(&extra);
        for f in frameworks {
            cmd.args(["-framework", f]);
        }
        let status = cmd.arg("-o").arg(out.join(name)).arg(src.join(file)).status();
        if !status.is_ok_and(|s| s.success()) {
            panic!("clang could not build {file} (install the Xcode command line tools)");
        }
    }
    // Only the macOS tests read these (env! in #[cfg(test)] code), so no build path ends up in a release binary.
    println!("cargo:rustc-env=COUCHLINK_TEST_INJECT_DYLIB={}", out.join("libcouchlink_inject.dylib").display());
    println!("cargo:rustc-env=COUCHLINK_TEST_INPUTPROBE={}", out.join("inputprobe").display());
}
