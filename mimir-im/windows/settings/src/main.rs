//! The Windows settings window: the macOS settings (一般・キー・モデル・辞書
//! ・学習・履歴) as a Tauri window over its own model-less `mimir-imserver`,
//! opened from the input indicator's menu (`mimir-tsf`). The page is
//! `ui/`; this side only relays the engine's methods and edits the user
//! dictionaries, which the engine reads as files. Design: `docs/windows.md`.
//!
//! macOS builds it only to try the window out; Linux builds a stub.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod dict;

#[cfg(any(windows, target_os = "macos"))]
mod app;

fn main() {
    #[cfg(any(windows, target_os = "macos"))]
    app::run();
    #[cfg(not(any(windows, target_os = "macos")))]
    eprintln!("mimir-settings: no window on this platform");
}
