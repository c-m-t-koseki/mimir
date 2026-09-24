fn main() {
    // The target's OS, not the host's: `cargo check --target
    // x86_64-pc-windows-msvc` on a Mac has to embed the Windows resources.
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target == "windows" || target == "macos" {
        tauri_build::build();
    }
}
