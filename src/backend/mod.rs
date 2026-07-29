//! Managed agent-backend runtime and protocol implementations.

pub mod control;
pub mod manifest;
pub mod opencode;
pub mod runtime;
pub mod supervisor;

pub use manifest::{RuntimeAsset, RuntimeManifest};
pub use runtime::{InstallSource, RuntimeManager, RuntimeStatus};
