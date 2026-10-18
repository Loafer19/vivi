//! Application wiring split out of `main.rs`.

pub mod cli;
pub mod debug;
#[cfg(target_os = "linux")]
pub mod egl;
pub mod keys;
pub mod panel;
pub mod session;
pub mod startup;
pub mod wiring;
