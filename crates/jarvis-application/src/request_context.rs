//! Server-derived request context.
//!
//! Trusted context is never assembled from ordinary request-body fields (see
//! the common contract conventions). [`RequestContext`] can therefore only be
//! built by trusted code after authentication and workspace resolution: it
//! serializes for audit and diagnostics, but deliberately does not implement
//! `Deserialize`, so it cannot be read back in from an untrusted body.

use jarvis_domain::ids::{CorrelationId, PrincipalId, RequestId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;
use serde::Serialize;

use crate::cancellation::CancellationScope;

/// How strongly the caller's identity was proven.
///
/// This is a closed, JARVIS-owned value because it is produced by the local
/// authentication core, not by an evolving external provider. Adapters that
/// receive an assurance claim from a provider must map it explicitly rather
/// than passing an unknown string through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationAssurance {
    /// No verified identity; only unauthenticated-safe operations are allowed.
    Guest,
    /// A verified local credential at the standard level.
    Standard,
    /// A verified identity that completed a step-up challenge.
    Elevated,
}

/// The interface a request arrived on.
///
/// Channel is part of authorization input, so it is a closed enum: a caller
/// cannot invent a more privileged channel string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestChannel {
    /// The `jarvis` command-line client.
    Cli,
    /// The desktop application.
    Desktop,
    /// The web client.
    Web,
    /// A mobile client.
    Mobile,
    /// A voice or telephony session.
    Voice,
    /// A local HTTP API client.
    Api,
    /// An internal daemon-initiated action with no external caller.
    Internal,
}

/// Trusted, server-derived context for one unit of work.
///
/// Every field is resolved by the server. Callers may request a workspace, but
/// they never supply the workspace that appears here.
#[derive(Debug, Clone, Serialize)]
pub struct RequestContext {
    /// Identifies this request.
    pub request_id: RequestId,
    /// Correlates this request with related requests and events.
    pub correlation_id: CorrelationId,
    /// The authenticated principal.
    pub principal_id: PrincipalId,
    /// How strongly `principal_id` was proven.
    pub assurance: AuthenticationAssurance,
    /// The resolved, authorized active workspace.
    pub workspace_id: WorkspaceId,
    /// The interface the request arrived on.
    pub channel: RequestChannel,
    /// The absolute instant after which the request must not start new work.
    pub deadline: Option<UtcTimestamp>,
    /// The cancellation scope for this request and its descendants.
    #[serde(skip)]
    cancellation: CancellationScope,
}

impl RequestContext {
    /// Builds context from already-trusted, server-resolved values.
    ///
    /// Callers must have authenticated the principal and resolved/authorized
    /// the workspace before calling this. The cancellation scope starts fresh;
    /// use [`with_cancellation`](Self::with_cancellation) to attach an existing
    /// scope so child work shares the request's cancellation.
    #[must_use]
    pub fn new(
        request_id: RequestId,
        correlation_id: CorrelationId,
        principal_id: PrincipalId,
        assurance: AuthenticationAssurance,
        workspace_id: WorkspaceId,
        channel: RequestChannel,
    ) -> Self {
        Self {
            request_id,
            correlation_id,
            principal_id,
            assurance,
            workspace_id,
            channel,
            deadline: None,
            cancellation: CancellationScope::new(),
        }
    }

    /// Attaches an existing cancellation scope.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationScope) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Sets the deadline after which the request must not start new work.
    #[must_use]
    pub fn with_deadline(mut self, deadline: UtcTimestamp) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Returns the cancellation scope for this request.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationScope {
        &self.cancellation
    }

    /// Returns whether `now` has reached or passed the deadline.
    ///
    /// A request with no deadline never expires. Comparisons use the injected
    /// `now` so the answer is deterministic under test.
    #[must_use]
    pub fn is_expired(&self, now: UtcTimestamp) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }
}

#[cfg(test)]
mod tests {
    use jarvis_domain::ids::{CorrelationId, PrincipalId, RequestId, WorkspaceId};
    use jarvis_domain::time::UtcTimestamp;

    use super::{AuthenticationAssurance, RequestChannel, RequestContext};

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d").expect("valid id")
    }

    fn principal() -> PrincipalId {
        PrincipalId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5e").expect("valid id")
    }

    fn context() -> RequestContext {
        RequestContext::new(
            RequestId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5f").expect("valid id"),
            CorrelationId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c60").expect("valid id"),
            principal(),
            AuthenticationAssurance::Standard,
            workspace(),
            RequestChannel::Cli,
        )
    }

    #[test]
    fn serialization_uses_canonical_strings_and_snake_case_enums() {
        let json = serde_json::to_string(&context()).expect("serialization succeeds");
        assert!(json.contains(r#""workspace_id":"018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d""#));
        assert!(json.contains(r#""assurance":"standard""#));
        assert!(json.contains(r#""channel":"cli""#));
        assert!(json.contains(r#""deadline":null"#));
        assert!(
            !json.contains("cancellation"),
            "the cancellation scope must never be serialized",
        );
    }

    #[test]
    fn deadline_expiry_is_computed_against_the_injected_now() {
        let deadline = UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid");
        let before = UtcTimestamp::parse("2026-09-20T11:59:59Z").expect("valid");
        let at = UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid");
        let after = UtcTimestamp::parse("2026-09-20T12:00:01Z").expect("valid");

        assert!(!context().is_expired(after), "no deadline never expires");

        let bounded = context().with_deadline(deadline);
        assert!(!bounded.is_expired(before));
        assert!(bounded.is_expired(at), "the deadline instant is expired");
        assert!(bounded.is_expired(after));
    }

    #[test]
    fn request_context_shares_its_cancellation_scope() {
        let context = context().with_cancellation(crate::cancellation::CancellationScope::new());
        let child = context.cancellation().child();

        context.cancellation().cancel();
        assert!(child.is_cancelled(), "child work must observe cancellation");
        assert!(context.cancellation().is_cancelled());
    }
}
