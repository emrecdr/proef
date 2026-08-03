//! Language server for proef feature files and macro packs.
//!
//! A second front-end over `proef-core`'s headless analysis: the same `Diag`
//! objects the CLI renders become LSP diagnostics, and the same binding/macro
//! relations power go-to-definition, completion, and references.

pub mod convert;
pub mod documents;
pub mod server;

pub use server::{ServerConfig, ServerError, Transport, run};
