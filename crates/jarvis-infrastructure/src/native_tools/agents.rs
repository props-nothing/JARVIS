//! The sub-agent tool: hand a self-contained task to another agent run and use its answer.
//!
//! `agents.delegate@1` is how one JARVIS run becomes a staff. The model names a task; the daemon starts a
//! **child run** (its own conversation, its own model calls), waits for it, and returns what it concluded. The
//! mechanism and its bounds — depth, breadth, deadline, cancellation — live in
//! `jarvis_application::run_service::delegation`; this module is the tool in front of it.
//!
//! **Classification, and why it is what it is.** The tool is `ReadOnly` / `Low` / `Allow`: the delegation itself
//! changes nothing in the world, and it does not widen authority, because the child is an ordinary run under the
//! same principal whose every tool call is classified, policy-checked and approved or refused on its own. A
//! delegated task that wants to write a file or send a message still stops at that tool's prompt. What
//! delegation does cost is model spend and fan-out, which the delegation bounds and the run budgets limit.
//!
//! **The caller is never taken from the arguments.** Who is delegating — workspace, principal, parent run — comes
//! from the pipeline's trusted [`ToolCaller`]; the arguments carry only the task text.
//!
//! The tool is offered only when the profile enables it (`[tools.agents] enabled = true`), so a default profile
//! cannot fan out model spend without the operator having said so.

use std::time::Duration;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::run_service::delegation::{
    DelegationError, DelegationRequest, DelegatorSlot,
};
use jarvis_application::tool_call::{ToolCaller, ToolExecutionError};
use jarvis_domain::error::DomainError;
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::run::state::RunState;
use jarvis_domain::tool::call::{ContentBlock, ToolResultBody};
use jarvis_domain::tool::classification::{
    ApprovalHint, Effect, ExecutionDefaults, Idempotency, Risk,
};
use jarvis_domain::tool::error_class::ToolErrorClass;

use super::{Definition, files};

/// The capability of the delegation tool.
pub const CAPABILITY: &str = "agents.delegate@1";

/// How long one delegation may take, end to end. The delegating run's own deadline still bounds it.
pub const DELEGATION_TIMEOUT_MS: u64 = 300_000;

/// The longest task, in bytes.
pub const MAX_TASK_BYTES: usize = 8 * 1024;

/// The input schema of `agents.delegate@1`.
pub const SCHEMA: &str = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":{"task":{"type":"string","minLength":1,"maxLength":8192,"description":"A self-contained task for the sub-agent. It cannot see this conversation, so include everything it needs."}},"required":["task"]}"#;

/// The reviewed definition of the delegation tool.
///
/// # Errors
///
/// Returns a construction refusal when the definition is inconsistent.
pub fn definition() -> Result<Definition, DomainError> {
    let mut built = files::build(
        CAPABILITY,
        SCHEMA,
        "Delegate to a sub-agent",
        "Hands one self-contained task to a separate sub-agent and returns its answer. The sub-agent works \
         independently with the same tools and the same permissions as you, cannot see this conversation, and \
         may itself delegate only a limited number of levels deeper. Use it to parallelise or isolate a piece \
         of work; do the simple things yourself.",
        vec![Effect::ReadOnly],
        Risk::Low,
        ApprovalHint::Allow,
        // A repeat would start a second child, so the pipeline must not retry it.
        Idempotency::None,
        (Sensitivity::Internal, Sensitivity::Confidential),
    )?;
    built.definition.execution = ExecutionDefaults::new(DELEGATION_TIMEOUT_MS, 1)?;
    Ok(built)
}

/// Returns whether `capability` names the delegation tool.
#[must_use]
pub fn is_agent_tool(capability: &str) -> bool {
    capability == CAPABILITY
}

/// Runs one delegation and shapes the child's outcome as the tool result.
///
/// # Errors
///
/// A refusal is mapped onto a tool error class the pipeline records and the model can read: a bound exceeded is
/// `LimitExceeded`, a wait that ran out is `Timeout`, a parent that was cancelled is `Cancelled`.
pub async fn execute(
    slot: &DelegatorSlot,
    caller: Option<ToolCaller>,
    arguments: &str,
    timeout_ms: u64,
    cancel: &CancellationScope,
) -> Result<ToolResultBody, ToolExecutionError> {
    let task = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| value.get("task")?.as_str().map(str::to_owned))
        .filter(|task| task.len() <= MAX_TASK_BYTES)
        .ok_or(ToolExecutionError::Failed(ToolErrorClass::SchemaInvalid))?;
    // A delegation needs a run to be a child of; a call with no run is not one this tool can serve.
    let caller = caller.ok_or(ToolExecutionError::Failed(ToolErrorClass::PermissionDenied))?;
    let delegator = slot
        .get()
        .ok_or(ToolExecutionError::Failed(ToolErrorClass::Unavailable))?;

    let outcome = delegator
        .delegate(
            DelegationRequest {
                caller,
                task: &task,
                timeout: Duration::from_millis(timeout_ms.min(DELEGATION_TIMEOUT_MS)),
            },
            cancel,
        )
        .await
        .map_err(refusal)?;

    let value = if outcome.state == RunState::Completed {
        serde_json::json!({
            "run_id": outcome.child.to_string(),
            "state": "completed",
            "answer": outcome.answer.unwrap_or_default(),
        })
    } else {
        // A child that ended failed or cancelled is information the delegating model can act on, not a fault of
        // this tool: it may retry the task differently or do it itself.
        serde_json::json!({
            "run_id": outcome.child.to_string(),
            "state": if outcome.state == RunState::Cancelled { "cancelled" } else { "failed" },
            "error_code": outcome.error_code,
        })
    };
    let block = ContentBlock::json(&value.to_string())
        .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))?;
    ToolResultBody::new(vec![block], None, Sensitivity::Confidential)
        .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))
}

/// Maps a delegation refusal onto what the pipeline records.
fn refusal(error: DelegationError) -> ToolExecutionError {
    match error {
        DelegationError::Cancelled => ToolExecutionError::Cancelled,
        DelegationError::Invalid => ToolExecutionError::Failed(ToolErrorClass::SchemaInvalid),
        DelegationError::DepthExceeded | DelegationError::TooManyChildren => {
            ToolExecutionError::Failed(ToolErrorClass::LimitExceeded)
        }
        DelegationError::TimedOut => ToolExecutionError::Failed(ToolErrorClass::Timeout),
        DelegationError::ParentGone => ToolExecutionError::Failed(ToolErrorClass::Conflict),
        DelegationError::Unavailable => ToolExecutionError::Failed(ToolErrorClass::Unavailable),
        DelegationError::Storage => ToolExecutionError::Failed(ToolErrorClass::ProviderError),
    }
}

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
