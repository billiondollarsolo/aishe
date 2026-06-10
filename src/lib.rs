//! aishe library crate. The binary (`src/main.rs`) is a thin REPL driver over
//! these modules; exposing them here also lets the integration tests in
//! `tests/` exercise the internals directly.

pub mod commands;
pub mod completer;
pub mod config;
pub mod context;
pub mod dispatcher;
pub mod executor;
pub mod fuzzy;
pub mod highlight;
pub mod history_expand;
pub mod integration;
pub mod modes;
pub mod prompt;
pub mod providers;
pub mod pty;
pub mod safety;
pub mod theme;
pub mod validator;
