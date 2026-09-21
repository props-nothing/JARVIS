//! Core JARVIS domain types and ports.
//!
//! This crate owns identity, time, and error primitives that every other JARVIS
//! layer depends on. It depends only on standard-library types plus two
//! narrowly justified value/time crates (`uuid` and `jiff`, per the Foundation
//! evidence note). It must not depend on `Axum`, `SQLx`, OS adapters, or any
//! provider SDK.
#![forbid(unsafe_code)]

pub mod clock;
pub mod error;
pub mod ids;
pub mod time;
