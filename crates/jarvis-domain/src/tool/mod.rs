//! The canonical tool fabric's domain layer: identity, classification, and definitions.
//!
//! This module is the typed form of `docs/contracts/tool-contract.md` plus the definition
//! half of `docs/architecture/tool-fabric.md`, and it exists to make two of that
//! architecture's rules structural rather than documented:
//!
//! - **A display name is not an identity.** [`identity::ToolIdentity`] has no name field, so
//!   an authorization decision cannot be made against an alias that can be re-pointed. That
//!   is `ACC-024`'s rule: replacing an implementation behind the same name must not inherit
//!   the original's approval, and the only way to guarantee that is for the name to be absent
//!   from the thing approvals bind to.
//! - **Classification comes from trusted configuration.** [`classification::Effect`],
//!   [`classification::Risk`], and [`classification::ApprovalHint`] parse only their closed
//!   sets, so an unknown value is refused instead of defaulting to a permissive one.
//!
//! The module deliberately stops at the definition and the registry. Policy evaluation, approvals,
//! execution, and the call ledger are separate concerns with their own TODOs (`TLS-003` through
//! `TLS-006`), and folding them in here would put the executor's I/O inside a value type.
//!
//! **The registry holds no grants.** `TLS-002` asks for discovery "independently from grants", and
//! the tool fabric states the rule it is protecting: "Tool discovery never grants execution
//! permission" and "Selection affects model context, not authorization at execution time". So
//! [`registry::ToolRegistry`] answers only *which definitions exist for this scope*, and a caller
//! that wants to know whether a tool may be **called** has to ask the policy layer — which does not
//! exist yet, and whose absence is visible as an absent function rather than a defaulted `true`.

pub mod approval;
pub mod call;
pub mod classification;
pub mod definition;
pub mod discovery;
pub mod error_class;
pub mod identity;
pub mod ledger;
pub mod policy;
pub mod registry;

pub use approval::{
    AllowedChannels, ApprovalActor, ApprovalChannel, ApprovalPreview, ApprovalScopeKind,
    ApprovalState, ApprovalTransitionRecord, ApprovalVersion, DurableApproval,
    MAX_APPROVAL_CHANNELS, MAX_CHANNEL_BYTES, MAX_PREVIEW_ITEMS, MAX_PREVIEW_TEXT_BYTES,
    MAX_SUMMARY_BYTES, PreviewItem,
};
pub use call::{
    ContentBlock, MAX_ARGUMENT_BYTES, MAX_PROVIDER_REFERENCE_BYTES, MAX_RESULT_ARTIFACTS,
    MAX_RESULT_BLOCKS, MAX_RESULT_BYTES, ResultPayload, ToolArguments, ToolCallIntent,
    ToolResultBody,
};
pub use classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, MAX_TOOL_ATTEMPTS,
    MAX_TOOL_EFFECTS, MAX_TOOL_SCOPES, MAX_TOOL_TIMEOUT_MS, Risk, Scope,
};
pub use definition::ToolDefinition;
pub use discovery::{
    DISCOVERY_TTL_DAYS, DiscoveredCatalog, DiscoveryCache, MAX_DISCOVERY_ENTRIES, catalog_for,
};
pub use error_class::{RetryPosture, ToolErrorClass};
pub use identity::{
    MAX_TOOL_MAJOR, MAX_TOOL_SEGMENT_BYTES, SchemaFingerprint, SourceKind, ToolCapability,
    ToolIdentity, ToolSource, ToolVersion,
};
pub use ledger::{
    InterruptedCallAction, LedgerEntry, LedgerOperation, MAX_IDEMPOTENCY_KEY_BYTES,
    MAX_LEDGER_SCAN, ReservationKey, ReservationOutcome, ToolCallLedger, ToolCallState,
    ToolCallTransition, ToolCallVersion, classify_interrupted,
};
pub use policy::{
    ApprovalRecord, DenyRule, Grant, GrantRef, PolicyDecision, PolicyInputs, PolicyOutcome,
    PolicyReason, PolicyRequest, evaluate,
};
pub use registry::{
    DiscoveryCacheKey, DiscoveryScope, ListVersion, MAX_REGISTERED_TOOLS, RegisteredTool,
    RegistrationOutcome, RegistrationRequest, ServerConfigId, ToolRegistry,
};

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tool_tests;

#[cfg(test)]
#[path = "registry_tests.rs"]
mod registry_tests;

#[cfg(test)]
#[path = "call_tests.rs"]
mod call_tests;

#[cfg(test)]
#[path = "policy_tests.rs"]
mod policy_tests;

#[cfg(test)]
#[path = "approval_tests.rs"]
mod approval_tests;

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod ledger_tests;
