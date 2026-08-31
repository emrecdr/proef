//! Feature handlers — each a thin read over the cached [`crate::analysis::Analysis`].

pub mod code_action;
pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod references;
