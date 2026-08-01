//! Behavior-preserving command-domain handlers used by the thin CLI binary.
//!
//! Clap owns parsing in `src/main.rs`; these modules own domain decisions and
//! rendering so command behavior can be tested without depending on the binary
//! entry point.

pub mod backend;
pub mod connection;
pub mod error_contract;
pub mod hints;
pub mod history;
pub mod json_contract;
pub mod runtime;
pub mod session;
pub mod settings;
pub mod status;
