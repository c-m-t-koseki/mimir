//! mimir-im: Japanese IME engine shared by the fcitx5 (Linux) and
//! macOS frontends.
//!
//! - fcitx5 C FFI lives in the separate `mimir-fcitx5` crate.
//! - The macOS stdio JSON-RPC server lives in [`server`] and is built
//!   as the `mimir-imserver` binary, bundled inside `mimir-macos`.

pub mod config;
pub mod core;
pub mod server;

pub use core::engine::{EngineAction, EngineResult, InputMethodEngine};
pub use core::keycode::{KeyEvent, KeyModifiers, Keysym};
pub use core::state::InputState;
