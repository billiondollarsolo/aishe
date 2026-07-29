//! Narrow OpenCode v1 REST/SSE adapter.

pub mod adapter;
pub mod client;
pub mod config;
pub mod mapper;
pub mod session;
pub mod sse;

pub use adapter::OpenCodeBackend;
pub use client::{OpenCodeClient, OpenCodeConnection, PromptNotAdmitted};
