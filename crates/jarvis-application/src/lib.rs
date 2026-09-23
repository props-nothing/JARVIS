//! Application orchestration over JARVIS domain ports.
#![forbid(unsafe_code)]
// See `jarvis-domain`'s crate root: a doc link to a nonexistent symbol is indistinguishable from
// a resolving one until something checks, so unresolved links are denied rather than warned.
#![deny(rustdoc::broken_intra_doc_links)]

pub mod cancellation;
pub mod context_assembly;
pub mod live_events;
pub mod model;
pub mod policy_service;
pub mod recovery;
pub mod repository;
pub mod request_context;
pub mod run_controller;
pub mod run_service;
pub mod testing;
