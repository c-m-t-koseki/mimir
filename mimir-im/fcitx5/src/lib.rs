//! mimir-fcitx5: fcitx5 addon for the Mimir Japanese IME
//!
//! This crate provides the C FFI interface consumed by the fcitx5 C++ addon
//! (`fcitx5-addon/src/mimir.cpp`). It wraps mimir-im's engine and exposes
//! a stable C ABI for input handling, preedit, candidates, and learning.

pub mod ffi;
