//! Omacull's engine: everything about a folder of raws that needs no window.

pub mod color;
pub mod cull;
pub mod develop;
pub mod disk;
pub mod faces;
pub mod folders;
pub mod image;
pub mod learn;
pub mod loader;
pub mod raw;
pub mod sidecar;
pub mod signals;
pub mod stacks;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod thumbs;
