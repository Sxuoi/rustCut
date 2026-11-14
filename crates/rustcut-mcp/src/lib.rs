//! MCP server (stdio) exposing RustCut's engine to AI agents. See the crate README.
#![warn(missing_docs)]

pub mod env;
pub mod export;
pub mod loud;
pub mod policy;
pub mod procguard;
pub mod projects;
pub mod rpc;
pub mod timeparse;
pub mod tools;
pub mod transcript;
