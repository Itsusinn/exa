//! Platform-independent client state and prediction.
//!
//! Network adapters deliver protocol messages; renderers consume this state.
//! This crate deliberately has no windowing, GPU, async runtime, or transport
//! dependency so the desktop and future browser hosts can share it.

pub mod distant;
pub mod voxel;
