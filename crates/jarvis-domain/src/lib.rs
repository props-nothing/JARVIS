//! Core JARVIS domain types and ports.
//!
//! This crate owns identity, time, and error primitives that every other JARVIS
//! layer depends on. It depends only on standard-library types plus narrowly
//! justified value/time crates (`uuid` and `jiff`, per the Foundation evidence
//! note). It must not depend on `Axum`, `SQLx`, OS adapters, or any provider SDK.
//!
//! The [`model`], [`run`], and [`context`] modules apply the same rule to their own
//! contracts: provider, run, and context semantics are expressed with domain types
//! and bounded text rather than with a JSON implementation or a provider SDK, so an
//! adapter normalizes at its own boundary.
#![forbid(unsafe_code)]

pub mod clock;
pub mod context;
pub mod error;
pub mod ids;
pub mod model;
pub mod run;
pub mod time;
