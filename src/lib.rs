//! llmsh library crate. The binary (`src/main.rs`) is a thin REPL driver over
//! these modules; exposing them here also lets the integration tests in
//! `tests/` exercise the internals directly.

pub mod config;
pub mod context;
pub mod dispatcher;
pub mod executor;
pub mod highlight;
pub mod modes;
pub mod prompt;
pub mod providers;
pub mod safety;
