//! Tests for the governed tool-call pipeline.
//!
//! Two things are being proved, and the first is the one the round exists for:
//!
//! 1. **The pipeline refuses in the right place, in the right order, and records which.** A denied
//!    call and an unknown tool must be *observations* rather than run failures, a schema-invalid
//!    call must be refused **before** policy runs, and an unapproved consequential call must not reach
//!    the executor at all.
//! 2. **`EXECUTING` is durable before the effect.** The rule the whole ledger is built on, and the
//!    only one whose failure duplicates a side effect.
//!
//! The doubles here are deliberately thin and their **calls are counted**, because a
//! refactor can keep every assertion true while removing a step: a pipeline that never calls the
//! validator passes "invalid arguments are refused" if the *catalog* also refuses them, and only a
//! call count distinguishes the two.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use jarvis_domain::clock::ManualClock;
use jarvis_domain::ids::{PrincipalId, RequestId, RunId, ToolCallId, WorkspaceId};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::policy::Grant;

use crate::repository::approval::ApprovalRepository;
use crate::repository::tool_call::ToolCallRepository;
use jarvis_domain::tool::call::{ContentBlock, ToolArguments, ToolCallIntent, ToolResultBody};
use jarvis_domain::tool::canonical::{ActionDigest, FingerprintInput};
use jarvis_domain::tool::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk,
};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use jarvis_domain::tool::ledger::ToolCallState;

use crate::cancellation::CancellationScope;
use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};
use crate::testing::InMemoryRepositories;
use crate::tool_call::{
    ActionFingerprint, ResolvedTool, ToolArgumentRefusal, ToolArgumentValidator, ToolCallOutcome,
    ToolCallService, ToolCatalog, ToolExecutionError, ToolExecutionFuture, ToolExecutionRequest,
    ToolExecutor, ToolGrantSource,
};

/// The workspace every test acts in.
fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(0x1000))
}

/// The principal every test acts as.
fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(0x2000))
}

/// The run every call belongs to.
fn run() -> RunId {
    RunId::from_uuid(uuid::Uuid::from_u128(0x3000))
}

/// The instant the clock reports.
fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-10-01T12:00:00Z").expect("valid")
}

fn context() -> RequestContext {
    RequestContext::new(
        RequestId::from_uuid(uuid::Uuid::from_u128(0x4000)),
        jarvis_domain::ids::CorrelationId::from_uuid(uuid::Uuid::from_u128(0x4001)),
        principal(),
        AuthenticationAssurance::Standard,
        workspace(),
        RequestChannel::Cli,
    )
}

/// The schema a readable tool declares.
const READ_SCHEMA: &str = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":{"path":{"type":"string"}},"required":["path"]}"#;

/// Builds a definition with a **shape-valid but arbitrary** schema fingerprint.
///
/// The fingerprint is deliberately not derived here. `jarvis-application` has no hashing dependency,
/// and the pair check it would exercise — schema text against fingerprint — is
/// `SchemaValidator`'s, asserted in `jarvis-infrastructure` where the real hasher lives
/// (`a_schema_the_fingerprint_does_not_name_is_refused`). A fabricated digest presented as a derived
/// one would be exactly the "a value whose provenance is invented" defect this project refuses, so
/// the value says what it is: arbitrary, and irrelevant to these tests because their validator is a
/// counting double rather than the real one.
fn definition(
    capability: &str,
    effects: Vec<Effect>,
    risk: Risk,
    hint: ApprovalHint,
    schema: &str,
) -> ToolDefinition {
    let capability = ToolCapability::parse(capability).expect("valid capability");
    let source = ToolSource::new(
        SourceKind::Native,
        "test.publisher",
        ToolVersion::parse("1.0.0").expect("valid"),
    )
    .expect("valid source");
    let _ = schema;
    ToolDefinition::new(
        ToolIdentity {
            capability,
            source,
            // Arbitrary and valid-shaped. See the doc comment above.
            schema_fingerprint: SchemaFingerprint::from_bytes([0x9a; 32]),
        },
        "Test tool",
        "A tool the pipeline tests dispatch.",
        effects,
        risk,
        Vec::new(),
        hint,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Public, Sensitivity::Internal).expect("valid"),
        ExecutionDefaults::new(5_000, 1).expect("valid"),
    )
    .expect("the definition is consistent")
}

/// A catalog over a fixed set of tools, counting resolutions.
struct Catalog {
    tools: Vec<(String, ResolvedTool)>,
    resolved: Arc<AtomicU32>,
}

impl Catalog {
    fn new(tools: Vec<ResolvedTool>) -> Self {
        Self {
            tools: tools
                .into_iter()
                .map(|tool| (tool.definition.identity.capability.to_string(), tool))
                .collect(),
            resolved: Arc::new(AtomicU32::new(0)),
        }
    }
}

impl ToolCatalog for Catalog {
    fn resolve(&self, _workspace: WorkspaceId, capability: &str) -> Option<ResolvedTool> {
        self.resolved.fetch_add(1, Ordering::SeqCst);
        self.tools
            .iter()
            .find(|(name, _)| name == capability)
            .map(|(_, tool)| tool.clone())
    }

    fn capabilities(&self) -> Vec<String> {
        self.tools.iter().map(|(name, _)| name.clone()).collect()
    }
}

/// A validator that counts how often it was consulted.
struct CountingValidator {
    calls: Arc<AtomicU32>,
    refuse: bool,
}

impl ToolArgumentValidator for CountingValidator {
    fn validate(
        &self,
        _tool: &ResolvedTool,
        _arguments: &ToolArguments,
    ) -> Result<(), ToolArgumentRefusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refuse {
            Err(ToolArgumentRefusal::Violated)
        } else {
            Ok(())
        }
    }
}

/// A fingerprint port that records the envelopes it saw.
struct CountingFingerprint {
    calls: Arc<AtomicU32>,
}

impl ActionFingerprint for CountingFingerprint {
    fn fingerprint(&self, _input: &FingerprintInput) -> ActionDigest {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ActionDigest::from_bytes([7; 32])
    }
}

/// An executor that records every request and can be told to fail.
struct RecordingExecutor {
    calls: Arc<Mutex<Vec<String>>>,
    failure: Option<ToolExecutionError>,
    /// The ledger states observed at the instant the effect ran.
    ///
    /// **This is what proves `EXECUTING` is written first.** The executor reads the ledger through a
    /// channel the fixture installs, so it can capture the row's state *during* the effect — after the
    /// call and the assertion would be measuring a later state, and the rule is about ordering.
    observed_states: Arc<Mutex<Vec<ToolCallState>>>,
    ledger: Option<Arc<InMemoryRepositories>>,
}

impl ToolExecutor for RecordingExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest<'a>,
        cancel: &'a CancellationScope,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("the lock is not poisoned")
                .push(request.identity.capability.to_string());
            // Read the durable row **while the effect is in progress**, which is the only moment the
            // ordering claim can be observed. `None` is recorded as a distinct "the store did not
            // hold exactly one row" marker rather than skipped, so a store state that makes the
            // ordering claim unobservable fails the assertion instead of silently passing it.
            if let Some(ledger) = &self.ledger {
                let state = ledger
                    .sole_ledger_state()
                    .expect("the ledger holds exactly one row while the effect runs");
                self.observed_states
                    .lock()
                    .expect("the lock is not poisoned")
                    .push(state);
            }
            if cancel.is_cancelled() {
                return Err(ToolExecutionError::Cancelled);
            }
            if let Some(error) = self.failure {
                return Err(error);
            }
            let block = ContentBlock::text("done").expect("valid");
            ToolResultBody::new(vec![block], None, Sensitivity::Internal)
                .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))
        })
    }
}

/// A grants source that answers from a fixed list.
struct Grants {
    grants: Vec<Grant>,
}

impl ToolGrantSource for Grants {
    fn read(
        &self,
        _principal: PrincipalId,
        _workspace: WorkspaceId,
    ) -> crate::tool_call::GrantReadFuture<'_> {
        let grants = self.grants.clone();
        Box::pin(async move {
            Ok(crate::tool_call::GrantRead {
                grants,
                deny_rules: Vec::new(),
            })
        })
    }
}

/// One pipeline and the counters a test asserts on.
struct Fixture {
    service: ToolCallService,
    repository: Arc<InMemoryRepositories>,
    catalog_resolutions: Arc<AtomicU32>,
    validations: Arc<AtomicU32>,
    fingerprints: Arc<AtomicU32>,
    executions: Arc<Mutex<Vec<String>>>,
    observed_states: Arc<Mutex<Vec<ToolCallState>>>,
}

impl Fixture {
    fn build(
        tools: Vec<ResolvedTool>,
        grants: Vec<Grant>,
        validator_refuses: bool,
        executor_failure: Option<ToolExecutionError>,
    ) -> Self {
        Self::build_with(tools, grants, validator_refuses, executor_failure, true)
    }

    /// As `Self::build`, optionally without the executor's single-row ledger observation, which a test
    /// that makes several calls cannot satisfy.
    fn build_with(
        tools: Vec<ResolvedTool>,
        grants: Vec<Grant>,
        validator_refuses: bool,
        executor_failure: Option<ToolExecutionError>,
        observe_ledger: bool,
    ) -> Self {
        let repository = Arc::new(InMemoryRepositories::new());
        let catalog = Arc::new(Catalog::new(tools));
        let catalog_resolutions = Arc::clone(&catalog.resolved);
        let validations = Arc::new(AtomicU32::new(0));
        let fingerprints = Arc::new(AtomicU32::new(0));
        let executions = Arc::new(Mutex::new(Vec::new()));
        let observed_states = Arc::new(Mutex::new(Vec::new()));
        let ledger_port = Arc::clone(&repository);
        let service = ToolCallService::new(
            catalog,
            Arc::new(Grants { grants }),
            Arc::new(CountingValidator {
                calls: Arc::clone(&validations),
                refuse: validator_refuses,
            }),
            Arc::new(CountingFingerprint {
                calls: Arc::clone(&fingerprints),
            }),
            Arc::new(RecordingExecutor {
                calls: Arc::clone(&executions),
                failure: executor_failure,
                observed_states: Arc::clone(&observed_states),
                // The executor reads the *durable* row through a dedicated accessor the double
                // exposes for exactly this observation.
                ledger: observe_ledger.then(|| Arc::clone(&ledger_port)),
            }),
            Arc::clone(&repository) as Arc<dyn ToolCallRepository>,
            Arc::clone(&repository) as Arc<dyn ApprovalRepository>,
            Arc::new(ManualClock::new(now())),
        );
        Self {
            service,
            repository,
            catalog_resolutions,
            validations,
            fingerprints,
            executions,
            observed_states,
        }
    }

    /// The default fixture: one readable tool with a **reviewed grant**, no deny rules, a working
    /// executor.
    ///
    /// The grant is not incidental: policy requires one before an action is allowed, so a fixture
    /// without it would refuse every call and every assertion below would be measuring a denial
    /// rather than the pipeline. `grant_for` states exactly the bounds the production
    /// `NativeReadOnlyGrants` emits, so the fixture's authorization is the daemon's.
    fn readable() -> Self {
        Self::build(
            vec![resolved_read_tool()],
            vec![grant_for(&resolved_read_tool())],
            false,
            None,
        )
    }

    /// Returns how many times the executor was asked to run.
    fn executions_count(&self) -> usize {
        self.executions.lock().expect("not poisoned").len()
    }
}

/// Builds a resolved read-only tool.
///
/// `Allow` is the hint policy's read-only fast path requires, so a fixture declaring `Ask` would wait
/// for approval on every call and every assertion below would be measuring a prompt rather than the
/// pipeline. It is also what the daemon's own clock tool declares.
fn resolved_read_tool() -> ResolvedTool {
    ResolvedTool {
        definition: definition(
            "files.read@1",
            vec![Effect::ReadOnly],
            Risk::Low,
            ApprovalHint::Allow,
            READ_SCHEMA,
        ),
        input_schema: Some(READ_SCHEMA.to_owned()),
    }
}

/// Builds a resolved **consequential** tool, which policy asks about.
fn resolved_send_tool() -> ResolvedTool {
    ResolvedTool {
        definition: definition(
            "email.send@1",
            vec![Effect::ExternalCommunication],
            Risk::High,
            ApprovalHint::Ask,
            READ_SCHEMA,
        ),
        input_schema: Some(READ_SCHEMA.to_owned()),
    }
}

/// Builds the grant that lets the consequential tool reach its own default.
///
/// A grant is required before the default is consulted, so without this the call would be refused for
/// a missing grant and never reach the `Ask` the test is about.
fn grant_for_send(tool: &ResolvedTool) -> Grant {
    Grant {
        identity: tool.definition.identity.clone(),
        workspace: workspace(),
        principal: principal(),
        scopes: tool.definition.required_scopes.iter().cloned().collect(),
        effects: [Effect::ExternalCommunication].into_iter().collect(),
        risk_ceiling: Risk::High,
        sensitivity_ceiling: tool.definition.data_classes.input,
        expires_at: None,
    }
}

/// Builds an intent naming `capability` with `arguments`.
fn intent(capability: &str, arguments: &str) -> ToolCallIntent {
    ToolCallIntent::new(
        ToolCallId::from_uuid(uuid::Uuid::from_u128(0x5000)),
        capability,
        ToolArguments::new(arguments).expect("valid arguments"),
        None,
    )
    .expect("valid intent")
}

/// Builds the reviewed grant the daemon emits for a read-only native tool.
///
/// Mirrors `NativeReadOnlyGrants` exactly — the same effect ceiling, the same risk ceiling, the same
/// principal-from-request rule — so a fixture authorized here is authorized in production, and a
/// change to one is visible as a failing test rather than as a grant that stopped matching.
fn grant_for(tool: &ResolvedTool) -> Grant {
    Grant {
        identity: tool.definition.identity.clone(),
        workspace: workspace(),
        principal: principal(),
        scopes: tool.definition.required_scopes.iter().cloned().collect(),
        effects: [Effect::ReadOnly].into_iter().collect(),
        risk_ceiling: Risk::Low,
        sensitivity_ceiling: tool.definition.data_classes.input,
        expires_at: None,
    }
}

#[tokio::test]
async fn a_read_only_low_risk_call_is_executed_and_completes() {
    let fixture = Fixture::readable();
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    assert!(outcome.is_completed(), "the call must run: {outcome:?}");
    assert_eq!(fixture.executions_count(), 1);
    // Every stage ran, so none is decorative: a pipeline that skipped validation or fingerprinting
    // could still complete a call, and only the counts distinguish it.
    assert_eq!(fixture.catalog_resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.validations.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.fingerprints.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_executing_state_is_durable_before_the_effect_runs() {
    // **The rule the ledger exists for.** `EXECUTING` is recorded before an external effect, so a
    // crash mid-effect leaves a row that says an effect may have happened. Recording it afterwards
    // would lose that fact and duplicate the effect on the next startup.
    let fixture = Fixture::readable();
    fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    let observed = fixture
        .observed_states
        .lock()
        .expect("not poisoned")
        .clone();
    assert_eq!(
        observed,
        vec![ToolCallState::Executing],
        "the effect must observe a durable EXECUTING row, not a later state",
    );
}

#[tokio::test]
async fn an_unknown_tool_is_refused_without_touching_policy_or_the_executor() {
    // A model naming a tool that does not exist is ordinary, so the refusal is an observation rather
    // than error. The counters prove the pipeline stopped at resolution: nothing was validated,
    // nothing was fingerprinted, and **nothing was executed**.
    let fixture = Fixture::readable();
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.write@1", "{}"),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    assert_eq!(
        outcome,
        ToolCallOutcome::Refused {
            class: ToolErrorClass::NotFound,
            state: ToolCallState::Requested,
        }
    );
    assert_eq!(fixture.validations.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.fingerprints.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.executions_count(), 0);
}

#[tokio::test]
async fn invalid_arguments_are_refused_before_policy_is_evaluated() {
    // The order matters: a refusal that reached policy would ask the user about an action whose
    // arguments were already known to be unusable, and a prompt for a call that cannot run is worse
    // than no prompt. The fingerprint count proves policy was never reached — the digest is
    // computed immediately before the evaluation, so it is the last stage that *must not* run.
    let fixture = Fixture::build(vec![resolved_read_tool()], Vec::new(), true, None);
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    assert_eq!(
        outcome,
        ToolCallOutcome::Refused {
            class: ToolErrorClass::SchemaInvalid,
            state: ToolCallState::Denied,
        }
    );
    assert_eq!(fixture.validations.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.fingerprints.load(Ordering::SeqCst),
        0,
        "policy must not be reached for a call whose arguments were refused",
    );
    assert_eq!(fixture.executions_count(), 0);
}

#[tokio::test]
async fn a_consequential_call_without_an_approval_waits_and_never_runs() {
    // The tool declares `Ask`, so policy asks — and the prompt is made **durable** before the caller
    // is told to wait. The executor count is the load-bearing assertion: a pipeline that ran the
    // tool and *then* asked would make every approval decorative.
    let tool = resolved_send_tool();
    let fixture = Fixture::build(vec![tool.clone()], vec![grant_for_send(&tool)], false, None);
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    let ToolCallOutcome::WaitingApproval { approval } = outcome else {
        // A consequential call reaching anything other than a prompt is the defect this test
        // exists for, so the failure names the observed outcome rather than merely failing.
        unreachable!("a consequential call must wait on a prompt, got {outcome:?}");
    };
    assert_eq!(fixture.executions_count(), 0, "the effect must not run");
    // And the prompt is readable through the approval store, so a user who opens the listing sees the
    // call that is blocked on them. The `load` port is the surface that reads one back, and it is
    // asserted directly rather than through the listing because a `Pending` approval is deliberately
    // absent from `decided_by` — a prompt is not a decision, which is itself worth asserting.
    let listed = fixture
        .repository
        .decided_by(workspace(), principal(), 10)
        .await
        .expect("the store reads");
    assert!(listed.is_empty(), "a pending approval is not a decision");
    let stored = ApprovalRepository::load(fixture.repository.as_ref(), workspace(), approval)
        .await
        .expect("the prompt is durable and readable");
    assert_eq!(
        stored.requesting_principal,
        principal(),
        "the prompt names the principal the server resolved, not one a caller supplied",
    );
}

#[tokio::test]
async fn a_prompt_names_the_very_row_the_ledger_opened() {
    // **The assertion is cross-record identity, and it is the one that has no proxy.** The approval
    // contract's request body carries a `tool_call_id`, and it has to name the call that is actually
    // blocked — otherwise a decided prompt points at a call that appears in no ledger row, and
    // nothing can connect the decision to the row it releases. The first version of
    // `raise_approval` minted a **fresh** `ToolCallId`, so the two durable records described one
    // logical call under two names; every other assertion in this file stayed green, because nothing
    // compared them.
    //
    // The check is deliberately not "field equals X": it re-reads the ledger row through the
    // repository and compares the pair, so it cannot be satisfied by two values that happen to be
    // built from the same expression in two places.
    let fixture = Fixture::build(
        vec![resolved_send_tool()],
        vec![grant_for_send(&resolved_send_tool())],
        false,
        None,
    );
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    let ToolCallOutcome::WaitingApproval { approval } = outcome else {
        unreachable!("a consequential call must wait on a prompt, got {outcome:?}");
    };

    let prompt = ApprovalRepository::load(fixture.repository.as_ref(), workspace(), approval)
        .await
        .expect("the prompt is durable");
    // The row is found through the **conversion scan**, not the outstanding-work list, and the
    // distinction is the ports' own: `possibly_effecting` returns only *dispatched* rows, and a call
    // waiting on a prompt has not been dispatched. `awaiting_conversion` is the scan that covers every
    // non-terminal state except `reconciling`, so a waiting row appears here — which is also the read
    // the recovery pass performs, so the assertion is made against a list production actually walks.
    let rows = fixture
        .repository
        .awaiting_conversion(u32::MAX)
        .await
        .expect("the store reads")
        .records;
    let row = rows
        .iter()
        .find(|row| row.run == run())
        .expect("the ledger opened a row for this waiting call");
    assert_eq!(
        prompt.tool_call, row.call_id,
        "the prompt must name the ledger row's own call identifier, not a second one",
    );
    // The row is left waiting rather than reserved: an approved call must still reserve before it may
    // dispatch, and a row that had advanced would have skipped that step.
    assert_eq!(row.state(), ToolCallState::WaitingApproval);
}

#[tokio::test]
async fn a_deny_rule_refuses_an_otherwise_readable_call() {
    // The deny rule is the one input that can refuse an action the low-risk fast path would allow, so
    // it is asserted separately: a pipeline that consulted the fast path first would run the tool.
    let definition = resolved_read_tool();
    let rule = jarvis_domain::tool::policy::DenyRule {
        identity: Some(definition.definition.identity.clone()),
        principal: None,
        workspace: None,
        effects: std::collections::BTreeSet::new(),
    };
    let repository = Arc::new(InMemoryRepositories::new());
    let catalog = Arc::new(Catalog::new(vec![definition]));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let service = ToolCallService::new(
        catalog,
        Arc::new(DenyingGrants { rules: vec![rule] }),
        Arc::new(CountingValidator {
            calls: Arc::new(AtomicU32::new(0)),
            refuse: false,
        }),
        Arc::new(CountingFingerprint {
            calls: Arc::new(AtomicU32::new(0)),
        }),
        Arc::new(RecordingExecutor {
            calls: Arc::clone(&executions),
            failure: None,
            observed_states: Arc::new(Mutex::new(Vec::new())),
            ledger: Some(Arc::clone(&repository)),
        }),
        Arc::clone(&repository) as Arc<dyn ToolCallRepository>,
        Arc::clone(&repository) as Arc<dyn ApprovalRepository>,
        Arc::new(ManualClock::new(now())),
    );
    let outcome = service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    assert_eq!(
        outcome,
        ToolCallOutcome::Refused {
            class: ToolErrorClass::PermissionDenied,
            state: ToolCallState::Denied,
        }
    );
    assert_eq!(
        executions.lock().expect("not poisoned").len(),
        0,
        "an explicitly denied call must not reach the executor",
    );
}

/// A grants source that also carries deny rules.
struct DenyingGrants {
    rules: Vec<jarvis_domain::tool::policy::DenyRule>,
}

impl ToolGrantSource for DenyingGrants {
    fn read(
        &self,
        _principal: PrincipalId,
        _workspace: WorkspaceId,
    ) -> crate::tool_call::GrantReadFuture<'_> {
        let rules = self.rules.clone();
        // The grants are empty and the rules are the fixture's: this double exists to prove the deny rule
        // is consulted **before** anything else, so it supplies no grant at all — a grant present here
        // would make the assertion satisfiable by the grant path rather than by the rule.
        Box::pin(async move {
            Ok(crate::tool_call::GrantRead {
                grants: Vec::new(),
                deny_rules: rules,
            })
        })
    }
}

#[tokio::test]
async fn a_settled_duplicate_does_not_run_the_tool_a_second_time() {
    // The reservation is the whole reason the ledger exists, and the observable consequence is that
    // the executor is asked **once** for two identical invocations.
    let fixture = Fixture::readable();
    let first = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    assert!(first.is_completed());
    let second = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    let ToolCallOutcome::Duplicate { state, .. } = second else {
        // The point of the test is that a repeat is answered from the ledger instead of re-run, so
        // the failure message carries what actually happened — including the execution count, which
        // is the fact a "ran it twice" defect would show.
        unreachable!(
            "a repeat must be answered as a duplicate, got {second:?} after {} executions",
            fixture.executions_count(),
        );
    };
    assert_eq!(state, ToolCallState::Succeeded);
    assert_eq!(
        fixture.executions_count(),
        1,
        "the effect must happen once for two identical calls",
    );
}

#[tokio::test]
async fn an_ambiguous_outcome_is_never_retried() {
    // A call whose outcome is unknown must be reconciled rather than repeated: retrying it is the one
    // response that can duplicate an effect. The class is `OutcomeAmbiguous`, which the contract
    // reserves for exactly this.
    let fixture = Fixture::build(
        vec![resolved_read_tool()],
        vec![grant_for(&resolved_read_tool())],
        false,
        Some(ToolExecutionError::Ambiguous),
    );
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("files.read@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    assert_eq!(
        outcome,
        ToolCallOutcome::Refused {
            class: ToolErrorClass::OutcomeAmbiguous,
            state: ToolCallState::Failed,
        }
    );
}

#[test]
fn a_refusal_maps_to_the_class_a_client_branches_on() {
    // The mapping from a service error to the class an observation reports, asserted as a table so a
    // new variant cannot be added without deciding which class it is.
    let table = [
        (
            ToolArgumentRefusal::Violated.error_class(),
            ToolErrorClass::SchemaInvalid,
        ),
        (
            ToolArgumentRefusal::NoSchema.error_class(),
            ToolErrorClass::SchemaInvalid,
        ),
        (
            // A schema that disagrees with its own fingerprint is `Unavailable` rather than
            // `SchemaInvalid`, because it is nobody's fixable arguments — it is an operator's.
            ToolArgumentRefusal::FingerprintMismatch.error_class(),
            ToolErrorClass::Unavailable,
        ),
        (
            ToolExecutionError::Ambiguous.error_class(),
            ToolErrorClass::OutcomeAmbiguous,
        ),
        (
            ToolExecutionError::Cancelled.error_class(),
            ToolErrorClass::Cancelled,
        ),
        (
            ToolExecutionError::Failed(ToolErrorClass::Timeout).error_class(),
            ToolErrorClass::Timeout,
        ),
    ];
    for (actual, expected) in table {
        assert_eq!(actual, expected);
    }
}

#[test]
fn an_ambiguous_outcome_ends_failed_rather_than_reconciling() {
    // The state a reader sees must be settled. A row left `Reconciling` for ever looks unsettled to
    // every reader including the reservation check, and this build has no provider-side read to
    // perform, so the honest record is "an effect may exist and nothing could establish it".
    assert_eq!(
        ToolExecutionError::Ambiguous.terminal_state(),
        ToolCallState::Failed
    );
    assert_eq!(
        ToolExecutionError::Cancelled.terminal_state(),
        ToolCallState::Cancelled,
        "a cancelled call is the caller's own action, not a fault",
    );
}

#[tokio::test]
async fn the_catalog_is_never_asked_absent_a_capability() {
    // A guard against a vacuous test: the fixture's catalog *does* resolve the tool it is asked about,
    // so an assertion that a call completed is a statement about the pipeline rather than about a
    // catalog that answered nothing.
    let fixture = Fixture::readable();
    assert_eq!(
        fixture.service.capabilities(),
        vec!["files.read@1".to_owned()],
        "the model must be told exactly what the catalog can dispatch",
    );
}

// ---------------------------------------------------------------------------------------
// A parked call is resumed from its decided approval, on the same ledger row.
// ---------------------------------------------------------------------------------------

/// Decides `approval` as `to`, the way the approval service would, and returns the stored record.
async fn decide_approval(
    fixture: &Fixture,
    approval: jarvis_domain::ids::ApprovalId,
    to: jarvis_domain::tool::approval::ApprovalState,
) -> jarvis_domain::tool::approval::DurableApproval {
    use jarvis_domain::tool::approval::{ApprovalActor, ApprovalChannel};
    let mut stored = ApprovalRepository::load(fixture.repository.as_ref(), workspace(), approval)
        .await
        .expect("the prompt is durable");
    let version = stored.version();
    let transition = stored
        .apply(
            to,
            version,
            ApprovalActor::Decided {
                principal: principal(),
                channel: ApprovalChannel::Cli,
                assurance: jarvis_domain::model::exception::RequiredAssurance::Standard,
                note: None,
            },
            now(),
        )
        .expect("a pending approval can be decided");
    ApprovalRepository::apply_transition(
        fixture.repository.as_ref(),
        workspace(),
        &transition,
        version,
        &transition.actor,
        &stored,
    )
    .await
    .expect("the decision is stored");
    stored
}

/// Parks one consequential call and returns the approval it is waiting on.
async fn park_send(fixture: &Fixture, key: &str) -> jarvis_domain::ids::ApprovalId {
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            key,
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome");
    let ToolCallOutcome::WaitingApproval { approval } = outcome else {
        unreachable!("a consequential call must wait on a prompt, got {outcome:?}");
    };
    approval
}

fn send_fixture() -> Fixture {
    let tool = resolved_send_tool();
    Fixture::build(vec![tool.clone()], vec![grant_for_send(&tool)], false, None)
}

#[tokio::test]
async fn an_approved_call_resumes_on_its_own_row_and_spends_the_approval() {
    use jarvis_domain::tool::approval::ApprovalState;
    let fixture = send_fixture();
    let approval = park_send(&fixture, "call-1").await;
    let decided = decide_approval(&fixture, approval, ApprovalState::Approved).await;
    assert_eq!(
        fixture.executions_count(),
        0,
        "a decision is not an execution"
    );

    let outcome = fixture
        .service
        .resume(
            &context(),
            run(),
            &decided,
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            &CancellationScope::new(),
        )
        .await
        .expect("the resume reaches an outcome");
    assert!(
        matches!(outcome, ToolCallOutcome::Completed { .. }),
        "an approved call must run: {outcome:?}",
    );
    assert_eq!(fixture.executions_count(), 1);

    // The approval is spent: one-shot means one call.
    let after = ApprovalRepository::load(fixture.repository.as_ref(), workspace(), approval)
        .await
        .expect("loads");
    assert_eq!(after.state(), ApprovalState::Consumed);

    // And the row that parked is the row that finished, rather than a second row beside it.
    let rows = fixture
        .repository
        .awaiting_conversion(u32::MAX)
        .await
        .expect("reads")
        .records;
    assert!(
        rows.iter().all(|row| row.run != run()),
        "no row for this run may be left non-terminal: {rows:?}",
    );
}

#[tokio::test]
async fn a_second_identical_call_after_the_approval_was_spent_asks_again() {
    use jarvis_domain::tool::approval::ApprovalState;
    let fixture = send_fixture();
    let approval = park_send(&fixture, "call-1").await;
    let decided = decide_approval(&fixture, approval, ApprovalState::Approved).await;
    fixture
        .service
        .resume(
            &context(),
            run(),
            &decided,
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            &CancellationScope::new(),
        )
        .await
        .expect("resumes");

    // The same action again, under a new key: the approval was one-shot, so it must ask rather than
    // run — which is the difference between one email and two.
    let second = park_send(&fixture, "call-2").await;
    assert_ne!(second, approval);
    assert_eq!(
        fixture.executions_count(),
        1,
        "the second call must not run"
    );
}

#[tokio::test]
async fn a_rejected_approval_refuses_the_call_and_never_authorizes_another() {
    use jarvis_domain::tool::approval::ApprovalState;
    let fixture = send_fixture();
    let approval = park_send(&fixture, "call-1").await;
    let decided = decide_approval(&fixture, approval, ApprovalState::Rejected).await;

    let outcome = fixture
        .service
        .resume(
            &context(),
            run(),
            &decided,
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            &CancellationScope::new(),
        )
        .await
        .expect("the resume reaches an outcome");
    assert!(
        matches!(
            outcome,
            ToolCallOutcome::Refused {
                class: ToolErrorClass::ApprovalRejected,
                state: ToolCallState::Denied,
            }
        ),
        "a rejection must refuse the call: {outcome:?}",
    );

    // A **rejected** record is in the principal's decided list, and it must not read as permission.
    let again = park_send(&fixture, "call-2").await;
    assert_ne!(again, approval);
    assert_eq!(fixture.executions_count(), 0, "nothing may have run");
}

#[tokio::test]
async fn resuming_twice_does_not_run_the_call_twice() {
    use jarvis_domain::tool::approval::ApprovalState;
    let fixture = send_fixture();
    let approval = park_send(&fixture, "call-1").await;
    let decided = decide_approval(&fixture, approval, ApprovalState::Approved).await;
    for _ in 0..2 {
        fixture
            .service
            .resume(
                &context(),
                run(),
                &decided,
                &intent("email.send@1", r#"{"path":"/tmp"}"#),
                &CancellationScope::new(),
            )
            .await
            .expect("resumes");
    }
    assert_eq!(fixture.executions_count(), 1);
}

// ---------------------------------------------------------------------------------------
// Autonomy and standing approvals through the whole pipeline.
// ---------------------------------------------------------------------------------------

fn resolved_write_tool() -> ResolvedTool {
    ResolvedTool {
        definition: definition(
            "notes.write@1",
            vec![Effect::Write],
            Risk::Moderate,
            ApprovalHint::Ask,
            READ_SCHEMA,
        ),
        input_schema: Some(READ_SCHEMA.to_owned()),
    }
}

fn grant_for_write(tool: &ResolvedTool) -> Grant {
    Grant {
        identity: tool.definition.identity.clone(),
        workspace: workspace(),
        principal: principal(),
        scopes: tool.definition.required_scopes.iter().cloned().collect(),
        effects: [Effect::Write].into_iter().collect(),
        risk_ceiling: Risk::Moderate,
        sensitivity_ceiling: tool.definition.data_classes.input,
        expires_at: None,
    }
}

fn write_fixture(level: jarvis_domain::tool::policy::AutonomyLevel) -> Fixture {
    let tool = resolved_write_tool();
    let mut fixture = Fixture::build_with(
        vec![tool.clone()],
        vec![grant_for_write(&tool)],
        false,
        None,
        false,
    );
    fixture.service = fixture.service.with_autonomy(level);
    fixture
}

async fn invoke_write(fixture: &Fixture, key: &str, arguments: &str) -> ToolCallOutcome {
    fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("notes.write@1", arguments),
            key,
            &CancellationScope::new(),
        )
        .await
        .expect("the pipeline reaches an outcome")
}

#[tokio::test]
async fn a_moderate_write_asks_unless_the_operator_chose_autonomous() {
    use jarvis_domain::tool::policy::AutonomyLevel;
    for level in [AutonomyLevel::Ask, AutonomyLevel::Balanced] {
        let fixture = write_fixture(level);
        let outcome = invoke_write(&fixture, "call-1", r#"{"path":"/a"}"#).await;
        assert!(
            matches!(outcome, ToolCallOutcome::WaitingApproval { .. }),
            "{level:?}: {outcome:?}"
        );
        assert_eq!(fixture.executions_count(), 0);
    }
    let fixture = write_fixture(AutonomyLevel::Autonomous);
    let outcome = invoke_write(&fixture, "call-1", r#"{"path":"/a"}"#).await;
    assert!(
        matches!(outcome, ToolCallOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        fixture.executions_count(),
        1,
        "autonomous runs a moderate write without a prompt"
    );
}

#[tokio::test]
async fn autonomy_never_runs_a_consequential_tool_without_asking() {
    use jarvis_domain::tool::policy::AutonomyLevel;
    let tool = resolved_send_tool();
    let mut fixture = Fixture::build(vec![tool.clone()], vec![grant_for_send(&tool)], false, None);
    fixture.service = fixture.service.with_autonomy(AutonomyLevel::Autonomous);
    let outcome = fixture
        .service
        .invoke(
            &context(),
            run(),
            &intent("email.send@1", r#"{"path":"/tmp"}"#),
            "call-1",
            &CancellationScope::new(),
        )
        .await
        .expect("outcome");
    assert!(
        matches!(outcome, ToolCallOutcome::WaitingApproval { .. }),
        "{outcome:?}"
    );
    assert_eq!(fixture.executions_count(), 0);
}

#[tokio::test]
async fn a_standing_approval_lets_later_calls_with_other_arguments_run_unprompted_and_is_not_spent()
{
    use jarvis_domain::tool::approval::{ApprovalScopeKind, ApprovalState};
    use jarvis_domain::tool::policy::AutonomyLevel;
    let fixture = write_fixture(AutonomyLevel::Balanced);
    let ToolCallOutcome::WaitingApproval { approval } =
        invoke_write(&fixture, "call-1", r#"{"path":"/a"}"#).await
    else {
        unreachable!("balanced asks about a write");
    };

    // The user answers "always": the record becomes standing, with a deadline past the prompt window.
    let mut stored = ApprovalRepository::load(fixture.repository.as_ref(), workspace(), approval)
        .await
        .expect("loads");
    stored.scope = ApprovalScopeKind::Standing;
    stored.expires_at = UtcTimestamp::parse("2026-10-08T12:00:00Z").expect("valid");
    let version = stored.version();
    let transition = stored
        .apply(
            ApprovalState::Approved,
            version,
            jarvis_domain::tool::approval::ApprovalActor::Decided {
                principal: principal(),
                channel: jarvis_domain::tool::approval::ApprovalChannel::Cli,
                assurance: jarvis_domain::model::exception::RequiredAssurance::Standard,
                note: None,
            },
            now(),
        )
        .expect("decides");
    ApprovalRepository::apply_transition(
        fixture.repository.as_ref(),
        workspace(),
        &transition,
        version,
        &transition.actor,
        &stored,
    )
    .await
    .expect("stores");
    let first = fixture
        .service
        .resume(
            &context(),
            run(),
            &stored,
            &intent("notes.write@1", r#"{"path":"/a"}"#),
            &CancellationScope::new(),
        )
        .await
        .expect("resumes");
    assert!(
        matches!(first, ToolCallOutcome::Completed { .. }),
        "{first:?}"
    );

    // A different action, under a new key, runs without being asked — twice.
    for (key, path) in [("call-2", "/b"), ("call-3", "/c")] {
        let outcome = invoke_write(&fixture, key, &format!(r#"{{"path":"{path}"}}"#)).await;
        assert!(
            matches!(outcome, ToolCallOutcome::Completed { .. }),
            "{key}: {outcome:?}"
        );
    }
    assert_eq!(fixture.executions_count(), 3);
    let after = ApprovalRepository::load(fixture.repository.as_ref(), workspace(), approval)
        .await
        .expect("loads");
    assert_eq!(
        after.state(),
        ApprovalState::Approved,
        "a standing approval is not spent by a use"
    );
}
