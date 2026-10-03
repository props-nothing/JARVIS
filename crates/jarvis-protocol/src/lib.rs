//! Versioned transport types at JARVIS process boundaries.
//!
//! These types are the wire contract between `jarvisd` and its clients. They are
//! versioned explicitly and serialization-tested, so a change is a deliberate
//! contract change rather than an incidental one.
#![forbid(unsafe_code)]
// See `jarvis-domain`'s crate root: a doc link to a nonexistent symbol is indistinguishable from
// a resolving one until something checks, so unresolved links are denied rather than warned.
#![deny(rustdoc::broken_intra_doc_links)]

pub mod approval;
pub mod discovery;
pub mod error;
pub mod memory;
pub mod policy;
pub mod run;
pub mod tool_grant;

pub use approval::{
    ApprovalDecisionResponse, ApprovalListView, ApprovalView, CancelApprovalRequest,
    DecideApprovalRequest, MAX_APPROVAL_PAGE, PreviewRowView,
};
pub use discovery::{DISCOVERY_SCHEMA_VERSION, DiscoveryFile, DiscoveryReject};
pub use error::{CODE_NAMESPACES, ErrorEnvelope, ErrorResponse, INTERNAL_CODE, is_owned_code};
pub use memory::{
    MemoryHitView, MemoryListView, MemorySearchView, MemoryView, RememberRequest, RememberedView,
};
pub use policy::{
    ActivePolicyResponse, DataPolicyView, EffectivePolicyResponse, EffectiveRouteView,
    PolicyRulesView, PutPolicyRequest, PutPolicyResponse, RejectedCandidateView, SourceLayerView,
};
pub use run::{
    CancelRunRequest, CreateRunRequest, CreateRunResponse, DEFAULT_RETRY_MAX_ATTEMPTS,
    MAX_CANCEL_REASON_BYTES, MAX_RUN_INPUT_BYTES, ModelPolicyRef, NATIVE_RUNTIME,
    RUN_CONTRACT_VERSION, RetryRequest, RunEventFrame, RunInput, RunLimitsView, RunLinks,
    RunUsageView, RunView, SseEvent, event_type,
};
pub use tool_grant::{
    DenyRuleCreatedView, DenyRuleView, GrantView, ToolDenyRuleListView, ToolGrantListView,
    WriteDenyRuleRequest, WriteToolGrantRequest,
};
