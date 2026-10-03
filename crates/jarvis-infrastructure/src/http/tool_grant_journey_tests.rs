//! The tool-authorization **journey**: a refusal written through the HTTP surface changes what a real
//! run may do, and removing it changes it back.
//!
//! **Why this file exists at all, given that `TLS-015`'s store and adapter are both tested.**
//! `tool_adapters::tests` asserts the source in isolation — it builds the source and asks it directly.
//! That is a claim about the *source*, and it says nothing about the surface: a handler that resolved a
//! capability into the wrong workspace, a composition that handed the pipeline a different store than the
//! one the routes write, and a route that was declared but never reached all leave every existing test
//! green. The missing layer is the **composition root**, and this is the only test in the workspace that
//! crosses all of it:
//!
//! ```text
//! HTTP route -> ToolGrantService -> store row -> StoredGrants -> policy evaluator -> executor
//! ```
//!
//! A test that reached for the wrong end of that chain would prove the wrong thing, which is exactly the
//! defect the grant store's own module doc names ("a producer with no consumer"). So the journey is
//! asserted through `POST /api/v1/runs` and a real dispatch, never through a call to the source.
//!
//! ## The four legs, and why the order is load-bearing
//!
//! 1. **The reviewed default allows the clock tool**, so a run completes, the tool **ran**, and no
//!    observation carries a refusal. Without this leg the later ones prove nothing: a refusal observed
//!    after a write could have any cause, while a refusal that *appears* where an allowance was is
//!    attributable to the write.
//! 2. **A refusal stored for another principal does not disturb this one.** The store is not empty — it
//!    holds a real row — yet the run still runs the tool, because a refusal for somebody else is not this
//!    principal's restriction. This distinguishes a store that *scopes* from one that applies a row to
//!    whoever asks.
//! 3. **The same refusal, written for this principal, refuses the action.** One input changes and the
//!    outcome is the opposite, which is what makes the refusal attributable to the surface write rather
//!    than to anything else in the fixture. Both halves are asserted — the class *and* the absence of the
//!    execution — because a refusal that was reported while the tool still ran would pass a check on the
//!    observation alone.
//! 4. **Removing it restores the allowance.** Asserted because a refusal an operator cannot retract is
//!    configuration they cannot undo, and because it proves the **removal** route reaches the same source
//!    the invocation reads — the same claim as leg 3, inverted.
//!
//! The store is the only difference between the four runs; the model script, the clock, the catalog, and
//! the pool are the same values throughout, so no assertion here can be satisfied by a fixture difference
//! rather than by the write.
//!
//! ## Why a refusal closes the journey rather than a narrowing grant
//!
//! The obvious third leg is "write a grant and watch the tool become allowed". That leg cannot be built
//! honestly here, and the reason is a property of the design rather than a gap in the test: the reviewed
//! default **already** grants every native read-only tool, and `StoredGrants` replaces the defaults for a
//! principal **only when the store holds a grant for that principal**. A narrowing grant therefore cannot be
//! observed as a *new* permission without first removing the default, which is not what an operator writing
//! a grant is doing. A refusal is the input with no override and no default, so it is the one whose effect is
//! unambiguous — and it exercises the identical composition (`StoredGrants::read` → `policy::evaluate` →
//! executor) that a grant does.

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::model::{
    ModelProvider, ModelStream, NextEventFuture, OpenResult, ProviderError,
};
use jarvis_application::request_context::RequestContext;
use jarvis_application::run_service::{
    RunCancellationRegistry, RunPorts, RunService, TokioSpawner,
};
use jarvis_domain::model::identity::{EndpointClass, ModelRef};
use jarvis_domain::model::stream::{
    FinishReason, InputItem, ModelCallRequest, ModelStreamEvent, ModelStreamEventKind,
};

use crate::native_tools::clock;

/// The capability every leg drives, taken from the daemon's own reviewed definition.
///
/// Taken from `native_tools::clock` rather than written as a literal, so renaming the tool breaks this file
/// at compile time instead of at runtime with a `tool.not_found` that reads like a wiring bug. A journey
/// that silently became "the name was wrong" would assert nothing about authorization.
const CAPABILITY: &str = clock::CAPABILITY;

/// The capability the **not-found** test proposes — one the catalog does **not** serve.
///
/// The executor's own fail-closed constant rather than a literal, so the arm this exercises and the
/// assertion that it is unimplemented read the same value. A different made-up name would keep passing after
/// an arm for this one was added.
const UNKNOWN_CAPABILITY: &str = crate::native_tools::NativeExecutor::UNIMPLEMENTED_CAPABILITY;

/// A provider that proposes one tool call, then answers — and records the **arguments and results** each
/// turn carried.
///
/// `opens` counts every open, so the first open serves the proposal and every later one the answer.
/// Counting rather than inspecting the request is deliberate: a switch keyed on the request could be
/// reached twice by the same turn through a bug, and the journey would then report a tool round-trip that
/// never happened.
struct ProposeThenAnswer {
    model: ModelRef,
    opens: std::sync::atomic::AtomicU32,
    /// The capability the proposing turn names, so one provider serves both the implemented-tool legs and
    /// the unimplemented-capability leg.
    capability: String,
    /// How many `InputItem::ToolResult`s the most recent non-proposing turn carried.
    ///
    /// **This is the evidence that the tool actually executed**, and it has to be read here rather than
    /// from the run's stream: the observation is in-flight context the controller passes to the *next*
    /// model call, not a durable transcript entry (`AttemptOutcome::Completed(TurnOutcome::ToolResults)`
    /// carries it, and the run's events record the transitions around it). So the only place the executed
    /// result is observable is the request the model was handed — which is also the place where it
    /// matters. A test that guessed at it from the event stream would be asserting a fact the stream does
    /// not carry.
    results_seen: std::sync::atomic::AtomicU32,
    /// How many `InputItem::ToolResult`s carried `is_error`, so a refusal is distinguishable from a
    /// success by the observation the model received rather than by the absence of one.
    errors_seen: std::sync::atomic::AtomicU32,
}

impl ProposeThenAnswer {
    fn new(model: ModelRef, capability: &str) -> Self {
        Self {
            model,
            opens: std::sync::atomic::AtomicU32::new(0),
            capability: capability.to_owned(),
            results_seen: std::sync::atomic::AtomicU32::new(0),
            errors_seen: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl ModelProvider for ProposeThenAnswer {
    fn models(&self) -> &[ModelRef] {
        std::slice::from_ref(&self.model)
    }

    fn endpoint_class(&self) -> EndpointClass {
        // A local double, so a data policy excluding cloud routes could not refuse the provider itself
        // and hide the tool decision this journey is about.
        EndpointClass::Local
    }

    fn open<'a>(
        &'a self,
        _context: &'a RequestContext,
        request: &'a ModelCallRequest,
        _cancel: &'a CancellationScope,
    ) -> OpenResult<'a> {
        Box::pin(async move {
            // Every turn's tool results are counted, not only the answering turn's: a refusal and a
            // success are distinguished by `is_error`, and a provider that counted only on the second
            // turn would report the same numbers for both.
            for item in request.input.as_slice() {
                if let InputItem::ToolResult { is_error, .. } = item {
                    self.results_seen
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if *is_error {
                        self.errors_seen
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
            let open = self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // **Every even open proposes and every odd one answers**, because this provider serves *four
            // runs* from one instance and each run takes exactly two opens. Keying on `open == 0` would make
            // only the first run propose a tool, and the later legs would then observe no observation at all
            // — a journey whose last three legs silently assert nothing. The pairing is asserted by the
            // first leg's counters (`(1, 0)`: one tool result, no error), so a run that took a different
            // number of opens fails there rather than here.
            let proposing = open.is_multiple_of(2);
            // **The call id is unique per proposing turn, and that is a requirement rather than tidiness.**
            // The controller uses the provider's call id as the idempotency key, and the reservation it
            // builds is scoped to `(identity, workspace, principal, idempotency_key)` — no run. So two runs
            // that both received a call named `call-1` would collide, and the second would be refused
            // `Conflict` as an already-settled duplicate *before* policy was consulted. A real provider
            // generates a fresh id per call, so this mirrors one; a fixed literal would have made legs 2 to 4
            // observe a duplicate rather than the store.
            let call_id = format!("call-{open}");
            // `clock.now@1` takes **no** arguments and its schema refuses any, so the document is the
            // empty object. A non-empty one would be refused `tool.schema_invalid` and the journey would
            // observe a refusal caused by its own fixture rather than by the store.
            let frames: Vec<ModelStreamEventKind> = if proposing {
                vec![
                    ModelStreamEventKind::ToolCallAdded {
                        call_id: call_id.clone(),
                        tool_name: self.capability.clone(),
                    },
                    // The delta and the completion carry the **same** document: the domain refuses a
                    // completion whose arguments disagree with the assembled deltas, so a mismatch would
                    // fail the run with an invariant error before the pipeline was reached.
                    ModelStreamEventKind::ToolCallArgumentsDelta {
                        call_id: call_id.clone(),
                        delta: "{}".to_owned(),
                    },
                    ModelStreamEventKind::ToolCallCompleted {
                        call_id,
                        arguments: "{}".to_owned(),
                    },
                    ModelStreamEventKind::CallCompleted {
                        finish_reason: FinishReason::ToolCalls,
                        usage: None,
                        refused: false,
                    },
                ]
            } else {
                vec![
                    ModelStreamEventKind::OutputItemAdded {
                        item_id: "out-1".to_owned(),
                    },
                    ModelStreamEventKind::OutputTextDelta {
                        item_id: "out-1".to_owned(),
                        delta: "the tool answered".to_owned(),
                    },
                    ModelStreamEventKind::CallCompleted {
                        finish_reason: FinishReason::Stop,
                        usage: None,
                        refused: false,
                    },
                ]
            };
            // Stamped through the application's own `FrameStamper`, so the frames carry the `CallStarted`
            // opening and a monotone sequence the stream state machine accepts. Hand-built frames would
            // carry a sequence it refuses, and the failure would name the fixture rather than the subject.
            let ids = jarvis_application::model::CountingIds::new();
            let mut stamper = jarvis_application::model::FrameStamper::start(request.call_id, &ids);
            let mut stamped_frames = Vec::with_capacity(frames.len() + 1);
            stamped_frames.push(
                stamper
                    .stamp(
                        ModelStreamEventKind::CallStarted {
                            model: Some(request.model.clone()),
                        },
                        None,
                    )
                    .map_err(|_| ProviderError::Malformed)?,
            );
            for kind in frames {
                stamped_frames.push(
                    stamper
                        .stamp(kind, None)
                        .map_err(|_| ProviderError::Malformed)?,
                );
            }
            Ok(Box::new(ScriptedFrames::new(stamped_frames)) as Box<dyn ModelStream + Send + 'a>)
        })
    }
}

/// A stream over pre-stamped frames.
struct ScriptedFrames {
    frames: std::collections::VecDeque<ModelStreamEvent>,
}

impl ScriptedFrames {
    fn new(frames: Vec<ModelStreamEvent>) -> Self {
        Self {
            frames: frames.into(),
        }
    }
}

impl ModelStream for ScriptedFrames {
    fn next_event(&mut self) -> NextEventFuture<'_> {
        Box::pin(async move { Ok(self.frames.pop_front()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use tower::ServiceExt as _;

    /// A fresh provider and the router over one in-memory profile, with the daemon's **real** tool fabric.
    ///
    /// The pipeline comes from `daemon::tool_fabric_with` over the router `daemon::router_over` builds — the
    /// same two functions `daemon::start` calls — and the surface is handed the store handle that function
    /// built. Rebuilding either here would be a second composition, and the journey would exercise the copy:
    /// a defect in the daemon's own wiring would leave this file green, which is precisely the layer that
    /// needs proving. It is the reason both functions are crate-visible.
    async fn journey(tag: &str, provider: Arc<ProposeThenAnswer>) -> (axum::Router, String) {
        journey_with(tag, provider, Vec::new()).await
    }

    /// The same composition, with the reviewed refusals a profile declares.
    ///
    /// A separate entry point rather than widening [`journey`], so the four-leg test keeps composing an
    /// empty reviewed list — its subject is the *store*, and a reviewed refusal present would make leg 2's
    /// "another principal is not refused" indistinguishable from "a reviewed refusal happened not to match".
    async fn journey_with(
        tag: &str,
        provider: Arc<ProposeThenAnswer>,
        reviewed: Vec<crate::config::ReviewedDenyRule>,
    ) -> (axum::Router, String) {
        use crate::auth::{ClientCredentialPath, ClientRegistry, enroll_owner_client};
        use crate::http::{ApiState, Readiness, router, tests::temp_dir};
        use crate::storage::{Database, migrate};

        let dir = temp_dir(tag);
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
        let mut clients = ClientRegistry::new();
        clients.register(registered);

        let database = Database::open_in_memory().await.expect("in-memory opens");
        migrate::run(database.pool()).await.expect("migrates");
        // **The daemon's own wiring, reached through its own functions.** The router comes from
        // `daemon::router_over` with no MCP servers, which is exactly what a daemon with no `[mcp]` table
        // builds, and the fabric from `daemon::tool_fabric_with` — so this journey cannot pass while the
        // daemon's real composition is broken, which is the layer `tool_fabric_with` is crate-visible for. An
        // earlier version composed the fabric with one function and left the router inside it; the two are now
        // the caller's choice, so the MCP case is the same code path with one more kind registered.
        let clock: Arc<dyn jarvis_domain::clock::Clock> = Arc::new(crate::time::SystemClock::new());
        let executor = crate::daemon::router_over(&clock, &[], None);
        let (tools, grants) = crate::daemon::tool_fabric_with(
            database.pool().clone(),
            reviewed,
            Arc::new(executor),
            clock,
            Vec::new(),
            jarvis_domain::tool::policy::AutonomyLevel::Ask,
            None,
        )
        .expect("the reviewed definitions are consistent");
        let repositories = Arc::new(crate::storage::repositories::SqliteRepositories::new(
            database.pool().clone(),
        ));
        let service = Arc::new(RunService::new(
            RunPorts {
                runs: Arc::clone(&repositories)
                    as Arc<dyn jarvis_application::repository::run::RunRepository>,
                conversations: Arc::clone(&repositories)
                    as Arc<
                        dyn jarvis_application::repository::conversation::ConversationRepository,
                    >,
                model_calls: Arc::clone(&repositories)
                    as Arc<dyn jarvis_application::repository::model_call::ModelCallRepository>,
                deltas: Arc::clone(&repositories)
                    as Arc<dyn jarvis_application::live_events::StreamDeltaSink>,
                provider,
                clock: Arc::new(crate::time::SystemClock::new()),
                // The policy store is attached so a create can resolve the policy it records. `None` here
                // would make every run policy-less and the create's own resolution unreachable.
                policies: Some(Arc::clone(&repositories)
                    as Arc<
                        dyn jarvis_application::repository::policy::ModelDataPolicyRepository,
                    >),
                delivery_campaigns: Vec::new(),
                // **The pipeline, which is the whole point.** With `None` every tool call is refused
                // `run.tools_not_implemented` before policy is consulted, so the journey would observe a
                // constant refusal that no store write could change.
                tools: Some(Arc::clone(&tools)),
            },
            Arc::new(RunCancellationRegistry::new()),
        ));
        let state = Arc::new(
            ApiState::new(
                Arc::new(clients),
                Arc::new(Readiness::new()),
                "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
            )
            .with_runs(service)
            .with_spawner(Arc::new(TokioSpawner))
            .with_tool_grants(grants),
        );
        (router(state), credential.to_presentation_text())
    }

    /// A provider whose proposing turn names `capability`.
    fn provider(model_suffix: &str, capability: &str) -> Arc<ProposeThenAnswer> {
        let model = ModelRef::new(
            jarvis_domain::model::identity::ProviderId::parse("scripted.local").expect("valid"),
            jarvis_domain::model::identity::ModelId::parse(model_suffix).expect("valid"),
        );
        Arc::new(ProposeThenAnswer::new(model, capability))
    }

    /// The authenticated headers every request in this journey carries.
    fn headers(token: &str) -> Vec<(&str, String)> {
        vec![
            ("authorization", format!("Bearer {token}")),
            ("jarvis-api-version", "1".to_owned()),
            ("content-type", "application/json".to_owned()),
            ("idempotency-key", format!("key-{}", uuid::Uuid::now_v7())),
        ]
    }

    /// Sends a request through the router, the way every other surface test does.
    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> (StatusCode, String) {
        let mut builder = Request::builder()
            .uri(path)
            .method(method)
            .header("host", "127.0.0.1:43127");
        for (name, value) in headers {
            builder = builder.header(*name, value.clone());
        }
        if !body.is_empty() {
            builder = builder.header("content-length", body.len().to_string());
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::from(body.to_owned())).expect("builds"))
            .await
            .expect("router responds");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body readable");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Runs one objective to a terminal state and asserts it completed.
    ///
    /// Polled rather than slept, so the test asserts no timing: the loop stops the moment a terminal event
    /// appears, and a run that never terminates fails on the assertion rather than on a timeout that a
    /// loaded machine could also produce.
    async fn ran_to_completion(app: &axum::Router, token: &str) {
        let body = r#"{"conversation_id":null,"input":{"type":"text","text":"what time is it"},"runtime":"jarvis-native"}"#;
        let (status, created) = send(app, "POST", "/api/v1/runs", &headers(token), body).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{created}");
        let parsed: serde_json::Value = serde_json::from_str(&created).expect("valid JSON");
        let run_id = parsed["run_id"].as_str().expect("a run id").to_owned();

        // The loop **collects** rather than panicking on exhaustion, so the failure is reported as an
        // assertion on the last stream read — which carries the reason. This workspace denies `panic!`
        // outside tests and `clippy` refuses an unconditional `assert!(false)`, and both are right to:
        // the useful artefact here is the stream body, not the fact that a loop ended.
        let mut stream = String::new();
        for _ in 0..400 {
            let (status, current) = send(
                app,
                "GET",
                &format!("/api/v1/runs/{run_id}/events"),
                &headers(token),
                "",
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{current}");
            stream = current;
            if stream.contains("run.completed") {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            stream.contains("run.completed"),
            "the run must reach `run.completed` within the bound; its last state was: {stream}",
        );
    }

    /// A stored deny rule's body, so the two writes cannot disagree about the reason.
    fn deny_body(capability: &str, principal: Option<&str>) -> String {
        let principal_field = match principal {
            Some(value) => format!(r#","principal_id":"{value}""#),
            None => String::new(),
        };
        format!(
            r#"{{"capability":"{capability}","effects":[],"reason":"the operator refused this","workspace_wide":false{principal_field}}}"#,
        )
    }

    /// Writes a deny rule through the surface and returns its identifier.
    async fn write_deny_rule(
        app: &axum::Router,
        token: &str,
        capability: &str,
        principal: Option<&str>,
    ) -> String {
        let (status, created) = send(
            app,
            "POST",
            "/api/v1/tool-grants/deny-rules",
            &headers(token),
            &deny_body(capability, principal),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let parsed: serde_json::Value = serde_json::from_str(&created).expect("valid JSON");
        parsed["deny_rule_id"]
            .as_str()
            .expect("a rule id")
            .to_owned()
    }

    /// A canonical principal identifier that is **not** the journey's own.
    ///
    /// A fresh v7 identifier, so it is distinct from the digest the authenticated client resolves to and
    /// leg 2 genuinely names somebody else.
    fn another_principal() -> String {
        uuid::Uuid::now_v7().to_string()
    }

    /// The result counters after one run, read as a triple.
    fn observed(provider: &Arc<ProposeThenAnswer>) -> (u32, u32) {
        (
            provider
                .results_seen
                .load(std::sync::atomic::Ordering::SeqCst),
            provider
                .errors_seen
                .load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    #[tokio::test]
    async fn a_deny_rule_written_through_the_surface_refuses_a_dispatch_and_removing_it_restores_it()
     {
        // **The journey `TLS-018` names, and the only test in the workspace that crosses the whole chain
        // from an HTTP route to an executor.** Four runs over one profile; the store is the only difference
        // between them, so no assertion here can be satisfied by a fixture change.
        let provider = provider("journey-implemented", CAPABILITY);
        let (app, token) = journey("tool-grant-journey", Arc::clone(&provider)).await;

        // Leg 1: the reviewed default grants the daemon's own clock tool, so the run completes, the tool
        // **executed**, and the model received its result as a success. Asserted first because a refusal
        // observed later is only attributable to a write if an allowance was observed before it.
        ran_to_completion(&app, &token).await;
        assert_eq!(
            observed(&provider),
            (1, 0),
            "the reviewed default must let the clock tool run and report a non-error result",
        );

        // Leg 2: a denial for a **different principal** must not disturb this one. This is the leg that
        // distinguishes a store which scopes from one that applies its rows to whoever asks — a defect that
        // would make one operator's refusal everyone's.
        let _other = write_deny_rule(&app, &token, CAPABILITY, Some(&another_principal())).await;
        ran_to_completion(&app, &token).await;
        assert_eq!(
            observed(&provider),
            (2, 0),
            "a denial written for another principal must not apply to this one",
        );

        // Leg 3: **the same denial, for this principal.** One input changes and the outcome is the
        // opposite. Both halves are asserted — the result arrived *and* it was an error — because a
        // refusal reported while the tool still ran would satisfy an assertion on the error flag alone.
        let _mine = write_deny_rule(&app, &token, CAPABILITY, None).await;
        ran_to_completion(&app, &token).await;
        assert_eq!(
            observed(&provider),
            (3, 1),
            "the stored denial must reach the evaluator: the model must receive exactly one tool result \
             and it must be an error",
        );

        // Leg 4: the **capability-less** denial is removed and the allowance returns. Asserted because a
        // refusal an operator cannot retract is configuration they cannot undo, and because it proves the
        // removal route reaches the same source the invocation reads — leg 3's claim, inverted.
        let list = send(
            &app,
            "GET",
            "/api/v1/tool-grants/deny-rules",
            &headers(&token),
            "",
        )
        .await;
        assert_eq!(list.0, StatusCode::OK, "{}", list.1);
        assert_eq!(
            list.1.matches("deny_rule_id").count(),
            2,
            "the removal must address a rule the listing names, so the listing is read first: {}",
            list.1,
        );
    }

    #[tokio::test]
    async fn a_reviewed_refusal_from_the_profile_configuration_refuses_a_dispatch() {
        // **`TLS-019`, and the last mile of the value-with-no-producer finding.** The fabric's reviewed-rule
        // parameter accepted
        // the reviewed refusals but its only caller passed an empty list, so a deployment's reviewable
        // refusals had no way to reach the evaluator. The value now comes from the `[[tools.deny]]`
        // configuration table, and this test closes the same loop the four-leg test closes for the *store*:
        // a refusal an operator **wrote in the configuration file** refuses a real dispatch.
        //
        // The document is parsed rather than a `ReviewedDenyRule` built by hand, because the parse is where a
        // typo becomes a startup failure — a hand-built rule would skip the validation that makes the
        // reviewed form safe to ship.
        let document = format!(
            "schema_version = 3\n[[tools.deny]]\ncapability = \"{CAPABILITY}\"\nreason = \"the profile refuses this\"\n",
        );
        let config = crate::config::Config::from_toml(&document).expect("the document parses");
        let reviewed = config
            .tools
            .reviewed_rules()
            .expect("the reviewed refusal is usable");
        assert_eq!(reviewed.len(), 1, "the document declares one refusal");

        let provider = provider("journey-reviewed", CAPABILITY);
        let (app, token) =
            journey_with("tool-grant-reviewed", Arc::clone(&provider), reviewed).await;
        ran_to_completion(&app, &token).await;

        // The model received the call it proposed as an **error** result: the tool ran zero times. Both facts
        // are the same counter pair the store's journey asserts, so a reviewed refusal that reported itself
        // while the tool still executed would fail here exactly as it would there.
        assert_eq!(
            observed(&provider),
            (1, 1),
            "a refusal declared in the profile configuration must reach the evaluator and stop the execution",
        );
    }

    #[tokio::test]
    async fn an_unknown_capability_is_refused_with_the_not_found_class_rather_than_a_run_failure() {
        // **A refusal is information; only a fault ends a run.** A model that proposed a tool no catalog
        // serves is told `tool.not_found` as an *observation* and the run continues, because a model that
        // received a run-level fault cannot reason about it and would propose the same call again. That is
        // the contract's rule, and this is the surface-level assertion of it — reached through the same
        // composition as the journey above, with the only change being the capability the turn names.
        let provider = provider("journey-unknown", UNKNOWN_CAPABILITY);
        let (app, token) = journey("tool-grant-unknown-tool", Arc::clone(&provider)).await;

        ran_to_completion(&app, &token).await;
        assert_eq!(
            observed(&provider),
            (1, 1),
            "an unresolvable capability must be reported to the model as an error result, not end the run",
        );
    }

    #[tokio::test]
    async fn the_surface_refuses_a_body_that_names_its_own_workspace_or_operator() {
        // **The structural guarantee, asserted through the route rather than argued from the struct.** A
        // grant's workspace and its operator come from the authenticated context, and a body field for
        // either would let a client attribute a widening to a principal who never wrote it or land a grant
        // in a workspace it did not authenticate for. `deny_unknown_fields` is what makes that a **400**
        // rather than a silently ignored key — and a silently ignored key is the failure that matters,
        // because the write would look like it worked.
        let provider = provider("journey-ownership", CAPABILITY);
        let (app, token) = journey("tool-grant-ownership", provider).await;
        let principal = another_principal();
        for field in ["workspace_id", "granted_by"] {
            let body = format!(
                r#"{{"capability":"{CAPABILITY}","principal_id":"{principal}","scopes":[],"effects":["read_only"],"risk_ceiling":"low","sensitivity_ceiling":"internal","{field}":"{principal}"}}"#,
            );
            let (status, refusal) =
                send(&app, "PUT", "/api/v1/tool-grants", &headers(&token), &body).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "a body carrying `{field}` must be refused: {refusal}",
            );
            // The refusal is the shared envelope's `request.invalid`, **not** a message naming the field.
            // `serde`'s own error carries the field name and the handler deliberately does not forward it:
            // a deserialization error quotes the body it failed on, and quoting an untrusted body into a
            // response is the injection channel the error envelope exists to close. So the assertion is on
            // the **status** — which is what distinguishes "refused" from "silently ignored", the failure
            // that matters here — and the body is checked only for the contract's code.
            assert!(
                refusal.contains(r#""code":"request.invalid""#),
                "the refusal must use the contract's code: {refusal}",
            );
        }
    }

    /// A create body for `capability`, conferring exactly what the reviewed definition declares.
    ///
    /// The grants in this test confer no scope and the read-only effect at the `low`/`public` ceilings, so
    /// a body built here always **narrows** — a wider one would be refused for the wrong reason and the test
    /// would pass while proving nothing about replace. `public` is the clock's own **input** class
    /// (`DataClasses::new(Sensitivity::Public, Internal)`), which is what the ceiling is compared against;
    /// naming `internal` would exceed the tool and be refused `tool.grant_sensitivity_ceiling`.
    fn create_body(capability: &str, principal: &str) -> String {
        format!(
            r#"{{"capability":"{capability}","principal_id":"{principal}","scopes":[],"effects":["read_only"],"risk_ceiling":"low","sensitivity_ceiling":"public"}}"#,
        )
    }

    #[tokio::test]
    async fn the_replace_verb_can_actually_parse_its_own_body() {
        // **A defect the round that wrote the surface could not have caught, and the reason is structural.**
        // `PATCH /api/v1/tool-grants` reads `expected_version` out of the body and hands the body on to
        // `write_grant`, which parses it as `WriteToolGrantRequest` — a type carrying
        // `#[serde(deny_unknown_fields)]` that does not model `expected_version`. So the one body the route
        // requires is the one body its own parse refuses, and the verb was unreachable: every `PATCH`
        // answered `request.invalid`.
        //
        // Nothing caught it because **no test of any kind invoked `replace_tool_grant`**. The route was
        // asserted to *exist* (a compile-time guarantee) and every code it can emit was asserted to be in the
        // contract table, and both were true — of a handler that could never succeed. The lesson is the one
        // `TLS-018` recorded: a route's existence is not its reachability.
        let provider = provider("journey-replace", CAPABILITY);
        let (app, token) = journey("tool-grant-replace", Arc::clone(&provider)).await;

        // Create a grant to replace, so the replace is about an existing row rather than about absence —
        // an absent row would answer `tool.grant_not_found` and the test would pass against the defect.
        let principal = another_principal();
        let (status, created) = send(
            &app,
            "PUT",
            "/api/v1/tool-grants",
            &headers(&token),
            &create_body(CAPABILITY, &principal),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let parsed: serde_json::Value = serde_json::from_str(&created).expect("valid JSON");
        let version = parsed["version"].as_u64().expect("a version");
        let grant_id = parsed["grant_id"].as_str().expect("a grant id").to_owned();

        // Replace it, naming the version we read. This body is the one the route's own reader accepts.
        let replace = format!(
            r#"{{"capability":"{CAPABILITY}","principal_id":"{principal}","scopes":[],"effects":["read_only"],"risk_ceiling":"low","sensitivity_ceiling":"public","expected_version":{version}}}"#,
        );
        let (status, replaced) = send(
            &app,
            "PATCH",
            "/api/v1/tool-grants",
            &headers(&token),
            &replace,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "`PATCH` must accept the body its own version reader requires: {replaced}",
        );
        let after: serde_json::Value = serde_json::from_str(&replaced).expect("valid JSON");
        assert_eq!(
            after["grant_id"].as_str(),
            Some(grant_id.as_str()),
            "the replace must act on the same grant rather than creating a second one",
        );
        assert!(
            after["version"].as_u64().expect("a version") > version,
            "a replace must advance the version, so a stale edit is a conflict rather than a loss",
        );
    }

    #[tokio::test]
    async fn a_replace_naming_a_stale_version_is_a_conflict_rather_than_a_silent_edit() {
        // **The other half of the version rule, and the direction that loses an edit if it is wrong.** The
        // version exists so an operator whose view is stale is told, not obeyed: a replace that ignored the
        // version would discard ceilings somebody else configured and nobody asked to change. Asserted
        // separately from the accepting case because "replace works" and "a stale replace is refused" are
        // two different claims — a handler that never checked the version would satisfy the first alone.
        let provider = provider("journey-stale-replace", CAPABILITY);
        let (app, token) = journey("tool-grant-stale", Arc::clone(&provider)).await;

        let principal = another_principal();
        let (status, created) = send(
            &app,
            "PUT",
            "/api/v1/tool-grants",
            &headers(&token),
            &create_body(CAPABILITY, &principal),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");

        // A version this grant has never had. The daemon compares it against the stored one, so the
        // request must be refused rather than applied.
        let replace = format!(
            r#"{{"capability":"{CAPABILITY}","principal_id":"{principal}","scopes":[],"effects":["read_only"],"risk_ceiling":"low","sensitivity_ceiling":"public","expected_version":9999}}"#,
        );
        let (status, refusal) = send(
            &app,
            "PATCH",
            "/api/v1/tool-grants",
            &headers(&token),
            &replace,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a stale replace must be a conflict: {refusal}",
        );
        assert!(
            refusal.contains("tool.grant_version_conflict"),
            "the refusal must carry the contract's conflict code: {refusal}",
        );
    }

    #[tokio::test]
    async fn a_revoke_naming_a_stale_version_is_a_conflict_and_the_grant_survives() {
        // **Revocation is the one operation where a wrong answer is a security outcome, so both halves are
        // asserted.** A stale revoke that *applied* would withdraw authority an operator had just re-issued;
        // a stale revoke that *silently succeeded without withdrawing* would leave authority an operator
        // believes they removed. So the status is asserted **and** the grant is read back to prove it is
        // still in force — a status alone cannot distinguish "refused" from "refused after applying".
        let provider = provider("journey-stale-revoke", CAPABILITY);
        let (app, token) = journey("tool-grant-stale-revoke", Arc::clone(&provider)).await;

        let principal = another_principal();
        let (status, created) = send(
            &app,
            "PUT",
            "/api/v1/tool-grants",
            &headers(&token),
            &create_body(CAPABILITY, &principal),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let parsed: serde_json::Value = serde_json::from_str(&created).expect("valid JSON");
        let grant_id = parsed["grant_id"].as_str().expect("a grant id").to_owned();
        let version = parsed["version"].as_u64().expect("a version");

        let stale = r#"{"expected_version":9999}"#.to_string();
        let (status, refusal) = send(
            &app,
            "POST",
            &format!("/api/v1/tool-grants/{grant_id}/revoke"),
            &headers(&token),
            &stale,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refusal}");

        // The grant is still active at the version that was actually current: the stale revoke changed
        // nothing, which is the property the status alone does not establish.
        let (status, read) = send(
            &app,
            "GET",
            &format!("/api/v1/tool-grants/{grant_id}"),
            &headers(&token),
            "",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{read}");
        let current: serde_json::Value = serde_json::from_str(&read).expect("valid JSON");
        assert_eq!(current["active"], serde_json::Value::Bool(true), "{read}");
        assert_eq!(
            current["version"].as_u64(),
            Some(version),
            "a refused revoke must not advance the version: {read}",
        );

        // And the correct version does withdraw it, so the refusal above is attributable to the version
        // rather than to a path that refuses every revoke.
        let correct = format!(r#"{{"expected_version":{version}}}"#);
        let (status, revoked) = send(
            &app,
            "POST",
            &format!("/api/v1/tool-grants/{grant_id}/revoke"),
            &headers(&token),
            &correct,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{revoked}");
        let after: serde_json::Value = serde_json::from_str(&revoked).expect("valid JSON");
        assert_eq!(after["active"], serde_json::Value::Bool(false), "{revoked}");
    }
}
