//! Servo embedding boundary.
//! Real Servo init lives behind `servo-embed` feature to keep default builds fast.
//! LTS pinned: servo =0.1.4 (see workspace). Monthly 0.8.x tracked post-M0.

#[cfg(feature = "servo-embed")]
pub mod headless;

#[cfg(feature = "servo-embed")]
pub use headless::{HeadlessPage, HeadlessSession, PageReport};

#[cfg(feature = "servo-embed")]
pub fn servo_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(not(feature = "servo-embed"))]
pub fn backend_name() -> &'static str {
    "stub-offscreen (servo-embed disabled for fast M0 builds)"
}
