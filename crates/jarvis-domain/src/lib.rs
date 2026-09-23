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
// A doc comment pointing at a symbol that does not exist reads exactly like one that resolves;
// the only difference is whether anything checks. `BRN-035` found 21 such links, two of them
// naming a type (`WireRunState`) that was never written — including the sentence explaining where
// the wire-state projection lives. Rustdoc reports these as warnings by default, which the rest of
// the workspace's gates ignored, so the links could rot silently. Denied here rather than left at
// `warn`, because "checked by a warning nothing reads" is the same as unchecked.
#![deny(rustdoc::broken_intra_doc_links)]

pub mod clock;
pub mod context;
pub mod error;
pub mod ids;
pub mod model;
pub mod run;
pub mod time;
