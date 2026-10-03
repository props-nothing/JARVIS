//! Tests for the delegation tool: its classification, how it maps a delegation's outcome and refusals, and the
//! one property that matters most — the identity a delegation runs under comes from the pipeline, never from the
//! arguments.

use std::sync::{Arc, Mutex};

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::run_service::delegation::{
    DelegationError, DelegationFuture, DelegationOutcome, DelegationRequest, Delegator,
    DelegatorSlot,
};
use jarvis_application::tool_call::{ToolCaller, ToolExecutionError};
use jarvis_domain::ids::{PrincipalId, RunId, WorkspaceId};
use jarvis_domain::run::state::RunState;
use jarvis_domain::tool::call::{ContentBlock, ToolResultBody};
use jarvis_domain::tool::classification::{ApprovalHint, Effect, Idempotency, Risk};
use jarvis_domain::tool::error_class::ToolErrorClass;

use super::{CAPABILITY, MAX_TASK_BYTES, definition, execute, is_agent_tool};

fn id(value: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(value)
}

fn caller() -> ToolCaller {
    ToolCaller {
        workspace: WorkspaceId::from_uuid(id(1)),
        principal: PrincipalId::from_uuid(id(2)),
        run: RunId::from_uuid(id(3)),
    }
}

/// A delegator that records what it was asked and answers with a scripted result.
struct Scripted {
    result: Result<DelegationOutcome, DelegationError>,
    seen: Mutex<Vec<(ToolCaller, String)>>,
}

impl Scripted {
    fn new(result: Result<DelegationOutcome, DelegationError>) -> Arc<Self> {
        Arc::new(Self {
            result,
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl Delegator for Scripted {
    fn delegate<'a>(
        &'a self,
        request: DelegationRequest<'a>,
        _cancel: &'a CancellationScope,
    ) -> DelegationFuture<'a> {
        self.seen
            .lock()
            .expect("lock")
            .push((request.caller, request.task.to_owned()));
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

fn slot_with(delegator: Arc<Scripted>) -> DelegatorSlot {
    let slot = DelegatorSlot::new();
    assert!(slot.bind(delegator));
    slot
}

fn completed(answer: &str) -> DelegationOutcome {
    DelegationOutcome {
        child: RunId::from_uuid(id(9)),
        state: RunState::Completed,
        answer: Some(answer.to_owned()),
        error_code: None,
    }
}

async fn run(
    slot: &DelegatorSlot,
    caller: Option<ToolCaller>,
    arguments: &str,
) -> Result<ToolResultBody, ToolExecutionError> {
    execute(slot, caller, arguments, 60_000, &CancellationScope::new()).await
}

fn json_of(body: &ToolResultBody) -> serde_json::Value {
    let ContentBlock::Json { value } = &body.content[0] else {
        unreachable!("the delegation tool returns one json block");
    };
    serde_json::from_str(value.as_str()).expect("the block parses")
}

#[test]
fn the_tool_is_classified_for_what_it_does_and_does_not_do() {
    let built = definition().expect("the definition is consistent");
    let definition = &built.definition;
    assert_eq!(definition.identity.capability.to_string(), CAPABILITY);
    assert!(is_agent_tool(CAPABILITY) && !is_agent_tool("files.read@1"));
    // It changes nothing itself and adds no authority: the child's own tool calls are each gated.
    assert_eq!(definition.effects.clone(), vec![Effect::ReadOnly]);
    assert_eq!(definition.risk, Risk::Low);
    assert_eq!(definition.default_approval, ApprovalHint::Allow);
    // A retry would start a second child, so the pipeline must not repeat it, and it may wait for minutes.
    assert_eq!(definition.idempotency, Idempotency::None);
    assert_eq!(definition.execution.timeout_ms, 300_000);
    assert!(built.input_schema.contains("\"required\":[\"task\"]"));
}

#[tokio::test]
async fn a_completed_child_returns_its_answer_and_its_run_id() {
    let delegator = Scripted::new(Ok(completed("42 notes")));
    let slot = slot_with(Arc::clone(&delegator));
    let body = run(&slot, Some(caller()), r#"{"task":"count the notes"}"#)
        .await
        .expect("completes");
    let value = json_of(&body);
    assert_eq!(value["state"], "completed");
    assert_eq!(value["answer"], "42 notes");
    assert_eq!(value["run_id"], RunId::from_uuid(id(9)).to_string());
}

#[tokio::test]
async fn a_child_that_ended_badly_is_information_for_the_model_not_a_tool_fault() {
    for (state, expected) in [
        (RunState::Failed, "failed"),
        (RunState::Cancelled, "cancelled"),
    ] {
        let slot = slot_with(Scripted::new(Ok(DelegationOutcome {
            child: RunId::from_uuid(id(9)),
            state,
            answer: None,
            error_code: Some("run.deadline_exceeded".to_owned()),
        })));
        let body = run(&slot, Some(caller()), r#"{"task":"x"}"#)
            .await
            .expect("a result");
        let value = json_of(&body);
        assert_eq!(value["state"], expected);
        assert_eq!(value["error_code"], "run.deadline_exceeded");
        assert!(value.get("answer").is_none());
    }
}

#[tokio::test]
async fn the_delegating_identity_comes_from_the_pipeline_never_from_the_arguments() {
    let delegator = Scripted::new(Ok(completed("ok")));
    let slot = slot_with(Arc::clone(&delegator));
    // The model writes a workspace, a principal and a parent of its own choosing; none may be used. The schema
    // refuses unknown properties, and even a document that slipped past it carries no identity into the call.
    let hostile =
        r#"{"task":"x","workspace":"other","principal":"root","parent_run_id":"someone-else"}"#;
    run(&slot, Some(caller()), hostile)
        .await
        .expect("the task is read");
    let seen = delegator.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].0,
        caller(),
        "the trusted caller is what the delegation ran as"
    );
    assert_eq!(seen[0].1, "x");
}

#[tokio::test]
async fn an_unusable_call_is_refused_before_anything_is_delegated() {
    let delegator = Scripted::new(Ok(completed("ok")));
    let slot = slot_with(Arc::clone(&delegator));
    let too_long = format!(r#"{{"task":"{}"}}"#, "a".repeat(MAX_TASK_BYTES + 1));
    for arguments in ["{}", r#"{"task":7}"#, "not json", too_long.as_str()] {
        let refused = run(&slot, Some(caller()), arguments)
            .await
            .expect_err("refused");
        assert_eq!(
            refused,
            ToolExecutionError::Failed(ToolErrorClass::SchemaInvalid),
            "{arguments:.40}"
        );
    }
    let no_run = run(&slot, None, r#"{"task":"x"}"#)
        .await
        .expect_err("refused");
    assert_eq!(
        no_run,
        ToolExecutionError::Failed(ToolErrorClass::PermissionDenied)
    );
    assert!(
        delegator.seen.lock().expect("lock").is_empty(),
        "nothing reached the delegator"
    );
}

#[tokio::test]
async fn an_unbound_slot_is_unavailable_rather_than_a_silent_success() {
    let refused = run(&DelegatorSlot::new(), Some(caller()), r#"{"task":"x"}"#)
        .await
        .expect_err("refused");
    assert_eq!(
        refused,
        ToolExecutionError::Failed(ToolErrorClass::Unavailable)
    );
}

#[tokio::test]
async fn each_refusal_maps_to_the_class_the_pipeline_records() {
    let table = [
        (
            DelegationError::Invalid,
            ToolExecutionError::Failed(ToolErrorClass::SchemaInvalid),
        ),
        (
            DelegationError::DepthExceeded,
            ToolExecutionError::Failed(ToolErrorClass::LimitExceeded),
        ),
        (
            DelegationError::TooManyChildren,
            ToolExecutionError::Failed(ToolErrorClass::LimitExceeded),
        ),
        (
            DelegationError::TimedOut,
            ToolExecutionError::Failed(ToolErrorClass::Timeout),
        ),
        (
            DelegationError::ParentGone,
            ToolExecutionError::Failed(ToolErrorClass::Conflict),
        ),
        (
            DelegationError::Unavailable,
            ToolExecutionError::Failed(ToolErrorClass::Unavailable),
        ),
        (
            DelegationError::Storage,
            ToolExecutionError::Failed(ToolErrorClass::ProviderError),
        ),
        (DelegationError::Cancelled, ToolExecutionError::Cancelled),
    ];
    for (error, expected) in table {
        let slot = slot_with(Scripted::new(Err(error)));
        let refused = run(&slot, Some(caller()), r#"{"task":"x"}"#)
            .await
            .expect_err("refused");
        assert_eq!(refused, expected, "{error:?}");
    }
}
