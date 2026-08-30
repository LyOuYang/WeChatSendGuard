fn main() {
    println!("cargo:rerun-if-changed=native/macos_bridge.h");
    println!("cargo:rerun-if-changed=native/macos_bridge.m");

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "macos" {
        return;
    }

    cc::Build::new()
        .file("native/macos_bridge.m")
        .flag("-fobjc-arc")
        .flag("-mmacosx-version-min=11.0")
        .warnings(true)
        .compile("wechat_send_guard_macos_bridge");

    for framework in ["AppKit", "ApplicationServices", "CoreGraphics", "Security"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
