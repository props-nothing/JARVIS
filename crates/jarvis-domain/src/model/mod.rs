//! The model gateway domain: provider/model identity, capability evidence, the
//! normalized model stream, and the model data policy.
//!
//! This module is the typed form of three accepted contracts —
//! `docs/contracts/model-stream.md`, `docs/contracts/model-data-policy.md`, and
//! the capability half of `docs/architecture/model-gateway.md` — so the rules they
//! state in prose are enforced by the types rather than restated in each caller.
//!
//! The four concepts the gateway architecture insists must never be used
//! interchangeably are kept apart here:
//!
//! - [`identity::ProviderId`] and [`identity::ModelId`] are different newtypes, so
//!   a provider cannot be passed where a model is required;
//! - [`identity::ModelRef`] carries both, because a model ID is only unique inside
//!   its provider;
//! - a *runtime* and a *route* are not model concepts at all and do not appear in
//!   this module.
//!
//! Three further rules are made structural rather than documented:
//!
//! - a capability claim requires [evidence](capability::Evidence), and an
//!   `UNVERIFIED`, `INFERRED`, or expired value cannot satisfy a hard requirement;
//! - [incremental delivery](capability::IncrementalDelivery) is a measurement of
//!   time-to-first-token **and** token spread, never a boolean;
//! - a [resolved policy](policy::ResolvedPolicy) merges layers from strongest to
//!   weakest, so a later layer can only narrow an earlier one.

pub mod capability;
pub mod exception;
pub mod identity;
pub mod policy;
pub mod routing;
pub mod stream;
