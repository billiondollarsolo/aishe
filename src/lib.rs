//! aishe library crate. The binary (`src/main.rs`) is a thin driver over these
//! modules; exposing them here also lets the integration tests in `tests/`
//! exercise the internals directly.

pub mod audit;
pub mod cache;
pub mod commands;
pub mod config;
pub mod context;
pub mod dispatcher;
pub mod executor;
pub mod fix;
pub mod fuzzy;
pub mod histlog;
pub mod index;
pub mod integration;
pub mod mcp;
pub mod modes;
pub mod overlay;
pub mod providers;
pub mod pty;
pub mod redact;
pub mod safety;
pub mod sandbox;
pub mod semhist;
pub mod session;
pub mod skills;
pub mod tools;
pub mod trust;
pub mod undo;
pub mod usage;
pub mod usagelog;
