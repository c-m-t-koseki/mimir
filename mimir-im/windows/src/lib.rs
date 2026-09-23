//! Mimir for Windows: a TSF text input processor (an in-process COM DLL)
//! in front of `mimir-imserver`, the same stdio JSON-RPC engine server the
//! macOS frontend runs.
//!
//! [`client`] (the server process and its protocol) and [`keymap`]
//! (virtual keys to the engine's XKB keysyms) are portable and tested on
//! any host; [`tsf`] is the Windows glue. Design and status:
//! `docs/windows.md`.

pub mod client;
pub mod keymap;

#[cfg(windows)]
mod tsf;
