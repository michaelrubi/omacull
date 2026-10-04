//! Omacull's engine: everything about a folder of raws that needs no window.

pub mod cull;
pub mod disk;
pub mod image;
pub mod loader;
pub mod raw;
pub mod sidecar;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod thumbs;
