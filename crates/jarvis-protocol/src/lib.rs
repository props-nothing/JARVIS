//! Versioned transport types at JARVIS process boundaries.
//!
//! These types are the wire contract between `jarvisd` and its clients. They are
//! versioned explicitly and serialization-tested, so a change is a deliberate
//! contract change rather than an incidental one.
#![forbid(unsafe_code)]

pub mod discovery;
pub mod error;
pub mod policy;
pub mod run;

pub use discovery::{DISCOVERY_SCHEMA_VERSION, DiscoveryFile, DiscoveryReject};
pub use error::{ErrorEnvelope, ErrorResponse};
pub use policy::{
    ActivePolicyResponse, DataPolicyView, EffectivePolicyResponse, EffectiveRouteView,
    PolicyRulesView, PutPolicyRequest, PutPolicyResponse, RejectedCandidateView,
};
pub use run::{
    CancelRunRequest, CreateRunRequest, CreateRunResponse, MAX_CANCEL_REASON_BYTES,
    MAX_RUN_INPUT_BYTES, ModelPolicyRef, NATIVE_RUNTIME, RUN_CONTRACT_VERSION, RunEventFrame,
    RunInput, RunLinks, RunView, SseEvent, event_type,
};
