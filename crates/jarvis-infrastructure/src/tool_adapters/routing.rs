//! Routing a call to the executor that implements it, by the source kind the tool declared.
//!
//! # Why a router is required rather than a convenience
//!
//! `ToolCallService::new` takes **one** `Arc<dyn ToolExecutor>`, and the daemon's composition passes the
//! native executor. So the moment a second source of tools exists —
//! an MCP server, a connector, a runtime — there is no way to dispatch to it: the pipeline's single slot is
//! already taken, and replacing it would stop the native tools working.
//!
//! That shape is deliberate in the port and this module is the answer to it. A pipeline with a *list* of
//! executors would be a second place dispatch is decided, and the ordering it applied would be invisible;
//! one executor that names its own routing decision is the same composition with the decision visible and
//! testable.
//!
//! # The routing key is the source **kind**, and the reason is that it is the trusted half
//!
//! A tool's `ToolSource` is server-reported or build-declared, and `ToolIdentity::authorizes` compares it —
//! so by the time a call reaches an executor the identity has been authorized and the source is the one the
//! grant named. The **kind** is the coarse part of that source: `Native`, `McpServer`, `Connector`,
//! `Runtime`, `Plugin`.
//!
//! Routing on the capability's *namespace* would be the tempting alternative — `mcp.read_file@1` looks like
//! it names its transport — and it is wrong twice over. A namespace is a contract's, not a transport's (the
//! MCP adapter reuses one constant for every server, by design), and it is not trusted the way a source is:
//! a tool called `mcp.something` from a native build would route to a server that never offered it.
//!
//! # An unrouted call is refused, and that is the fail-closed direction
//!
//! A source kind with no executor is `NotFound` rather than a fallback to the native executor. The fallback
//! is what a hurried version writes — "native is the common case" — and its failure is a *misroute*: a
//! server's tool name reaching the daemon's own clock or filesystem. `NotFound` is also the class an operator
//! can act on, because it says the daemon has no implementation for the tool rather than that the tool
//! misbehaved.

use std::collections::BTreeMap;
use std::sync::Arc;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{
    ToolExecutionError, ToolExecutionFuture, ToolExecutionRequest, ToolExecutor,
};
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::SourceKind;

/// Dispatches a call to the executor registered for the tool's source kind.
///
/// See the module doc for why the kind is the key and why an unrouted call is refused.
pub struct RoutingExecutor {
    /// One executor per source kind. A `BTreeMap` so the set is ordered and a test can assert on it.
    ///
    /// **At most one executor per kind**, enforced by the map rather than by a check: two executors claiming
    /// `McpServer` would leave the choice to insertion order, which is the invisible dispatch this module
    /// exists to prevent. A caller that needs two servers registers one executor over both — which is what
    /// `McpToolExecutor` is built to allow, since it takes a set of identities rather than one.
    by_kind: BTreeMap<SourceKind, Arc<dyn ToolExecutor>>,
}

impl std::fmt::Debug for RoutingExecutor {
    /// Reports which kinds are routed and how many, never the executors — a `Debug` rendering must not walk
    /// a port that could hold configuration.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RoutingExecutor")
            .field("routed", &self.by_kind.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl RoutingExecutor {
    /// Builds a router from the executors to route to.
    ///
    /// Built from pairs rather than from a builder, so the mapping is a value a caller can read and a test
    /// can assert. A **later pair with the same kind replaces an earlier one**, which is the honest
    /// behaviour for a map being built in order — but the composition never relies on it: a caller
    /// registering one kind twice is a defect this returns no error for and a test asserts against.
    #[must_use]
    pub fn new(executors: impl IntoIterator<Item = (SourceKind, Arc<dyn ToolExecutor>)>) -> Self {
        Self {
            by_kind: executors.into_iter().collect(),
        }
    }

    /// Returns the source kinds this router can dispatch.
    #[must_use]
    pub fn routed_kinds(&self) -> Vec<SourceKind> {
        self.by_kind.keys().copied().collect()
    }

    /// Returns how many source kinds are routed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_kind.len()
    }

    /// Returns whether nothing is routed.
    ///
    /// Present because a `len` without an `is_empty` is a lint, and because the answer is worth stating: an
    /// empty router refuses every call, which is a valid (if useless) configuration rather than a defect.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_kind.is_empty()
    }

    /// Returns the executor registered for a source kind.
    #[must_use]
    pub fn executor_for(&self, kind: SourceKind) -> Option<&Arc<dyn ToolExecutor>> {
        self.by_kind.get(&kind)
    }
}

impl ToolExecutor for RoutingExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest<'a>,
        cancel: &'a CancellationScope,
    ) -> ToolExecutionFuture<'a> {
        // The lookup happens here rather than inside an async block, so the borrow of `self` is resolved
        // before the future exists — which is what lets the returned future own the `Arc` it dispatches to
        // instead of borrowing the router.
        let Some(executor) = self.by_kind.get(&request.identity.source.kind) else {
            // **No fallback, and no panic.** `NotFound` is the class an operator can act on: it says the
            // daemon has no implementation for this tool, rather than blaming the tool for failing. Falling
            // back to another executor would put a server's tool name in front of the daemon's own
            // implementation, which is the misroute this whole module exists to make impossible.
            return Box::pin(
                async move { Err(ToolExecutionError::Failed(ToolErrorClass::NotFound)) },
            );
        };
        let executor = Arc::clone(executor);
        Box::pin(async move { executor.execute(request, cancel).await })
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
