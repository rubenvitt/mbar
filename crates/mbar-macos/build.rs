//! Link search path for private frameworks (SkyLight) on macOS targets.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-search=framework=/System/Library/PrivateFrameworks");
    }
}
