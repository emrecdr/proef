//! Engine-agnostic core of **proef**, the declarative multi-engine e2e test runner.
//!
//! This crate owns the full front-end pipeline (gherkin parse → bind → lower → emit),
//! the engine seam ([`engine::EngineFactory`] / [`engine::EngineSession`], ADR-0002),
//! the [`world::World`] variable scope (ADR-0005), the serde [`event::Event`] spine
//! (ADR-0008), and the fault taxonomy with stable exit codes (ADR-0009).
//!
//! # Core purity (sans-IO lite)
//!
//! The pipeline stages in this crate do no IO, read no clocks or environment, and
//! generate no randomness — `run_id`, timestamps, and env snapshots are **injected**
//! values. The single sanctioned persistence component is [`world::GlobalStore`],
//! whose load/save functions are invoked only from the orchestrating edge (the CLI),
//! never from pipeline code. This is what makes snapshot and property tests
//! bit-deterministic; do not break it for convenience.

pub mod analyze;
pub mod bind;
pub mod cancel;
pub mod diag;
pub mod emit;
pub mod engine;
pub mod error;
pub mod event;
pub mod fake;
pub mod feature;
pub mod html;
pub mod lower;
pub mod matcher;
pub mod pack;
pub mod provider;
pub mod report;
pub mod resolve;
pub mod runner;
pub mod step;
pub mod tags;
pub mod world;
