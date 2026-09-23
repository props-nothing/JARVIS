//! Infrastructure adapters that implement JARVIS domain ports.
//!
//! This crate binds domain ports to operating-system facilities. It is the only
//! place allowed to know that a real clock or UUID generator exists.
#![forbid(unsafe_code)]
// See `jarvis-domain`'s crate root: a doc link to a nonexistent symbol is indistinguishable from
// a resolving one until something checks, so unresolved links are denied rather than warned.
#![deny(rustdoc::broken_intra_doc_links)]

pub mod auth;
pub mod client;
pub mod config;
pub mod daemon;
pub mod diagnostics;
pub mod error;
pub mod http;
pub mod ids;
pub mod install;
pub mod lifecycle;
pub mod paths;
pub mod profile;
pub mod release;
pub mod service;
pub mod storage;
pub mod time;
