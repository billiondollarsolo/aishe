//! aishe library crate. The binary (`src/main.rs`) is a thin REPL driver over
//! these modules; exposing them here also lets the integration tests in
//! `tests/` exercise the internals directly.

pub mod audit;
pub mod cache;
pub mod commands;
pub mod completer;
pub mod config;
pub mod context;
pub mod dispatcher;
pub mod executor;
pub mod fuzzy;
pub mod ghost;
pub mod highlight;
pub mod histfilter;
pub mod history_expand;
pub mod integration;
pub mod mcp;
pub mod modes;
pub mod prompt;
pub mod providers;
pub mod pty;
pub mod redact;
pub mod safety;
pub mod session;
pub mod skills;
pub mod theme;
pub mod tools;
pub mod usage;
pub mod validator;
