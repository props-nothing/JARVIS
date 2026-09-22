# Agent and Runtime Model

Status: PROPOSED

## Two Layers

JARVIS distinguishes the durable run controller from the selected reasoning
runtime.

- The **run controller** owns identity, state, context references, policy,
  approvals, tool outcomes, public events, cancellation, and completion.
- The **runtime** proposes reasoning steps, model calls, tool intents, and a final
  response according to its own strategy.

The controller remains authoritative even when the native runtime is selected.

## Native Run State Machine

```mermaid
stateDiagram-v2
    [*] --> Received
    Received --> ContextBuilding
    ContextBuilding --> Planning
    Planning --> AwaitingModel
    AwaitingModel --> ExecutingTool: allowed tool intent
    AwaitingModel --> AwaitingApproval: approval required
    AwaitingApproval --> ExecutingTool: approved
    AwaitingApproval --> Observing: rejected/expired
    ExecutingTool --> Observing
    Observing --> Planning: more work
    Observing --> Waiting: event/time dependency
    Waiting --> Planning: resumed
    Planning --> Responding: answer ready
    Responding --> Completed
    Received --> Failed
    ContextBuilding --> Failed
    Planning --> Failed
    AwaitingModel --> Failed
    ExecutingTool --> Failed
    Waiting --> Failed
    AwaitingApproval --> Cancelled
    AwaitingModel --> Cancelled
    ExecutingTool --> Cancelled
    Waiting --> Cancelled
    Failed --> [*]
    Cancelled --> [*]
    Completed --> [*]
```

State names are domain concepts, not UI strings. A transition records actor,
reason, expected prior version, timestamp, and correlation metadata.

### Implemented evidence (`BRN-005`)

`jarvis_domain::run` implements this state machine, so the edges above are
enforced rather than restated. `RunState::allowed_targets` is the diagram
transcribed, and `RunLifecycle::apply` is the single place a change is decided:
the state is **private**, so an edge the diagram does not contain cannot be
reached by assigning to a field.

Four rules are structural rather than documented:

- **Terminal states absorb.** `Completed`, `Failed`, and `Cancelled` have no
  successors, so a late worker cannot resurrect a finished run — and a second
  terminal event, which the local control API forbids, is unreachable.
- **A transition carries provenance.** `RunTransition` supplies the actor, the
  bounded reason, the expected prior version, the instant, and an optional
  correlation ID; `RunTransitionRecord` describes what was *applied*, including
  both the prior and the resulting version, because "the state changed" is not
  auditable on its own.
- **Optimistic concurrency, and a stated check order.** The refusals are ordered
  terminal → version → edge so a caller receives the most specific true answer: an
  illegal edge computed from a stale view may be legal from the current state, so a
  stale caller is told its view is stale first. `RunVersionConflict` is the one
  domain error that is **retryable**, because re-reading and recomputing is exactly
  what stating an expected version is for.
- **Waiting is explicit and distinct from failure.** `AwaitingApproval` and
  `Waiting` are named states, so a run parked on a dependency does not look like a
  run that is merely not progressing.

**Two questions this transcription surfaced, recorded rather than resolved:**

1. The diagram gives `Responding` exactly one successor (`Completed`), so a
   provider failure *while producing the final answer* is not expressible: the
   refusal is pinned by
   `an_edge_absent_from_the_diagram_is_refused`. Either failure during responding
   must be reachable (a diagram change) or it must be defined to surface before
   `Responding` is entered (a controller rule) — the architecture owner decides.
2. The client-visible states in the
   [local control API](../contracts/local-control-api.md) are a **coarser** set
   than the controller's, and this module holds no projection function on purpose
   (the sentence above forbids domain states doubling as UI strings). The mapping
   is therefore `BRN-007`'s to implement at the API boundary, where the wire
   contract is owned, and it must be total over the non-terminal states.

## Durable Run Record

A run minimally tracks:

- run, session, conversation, principal, workspace, and parent IDs;
- objective and public input references;
- selected runtime and version;
- state and optimistic version;
- context manifest reference;
- current plan summary and step cursor;
- pending approval, timer, event, or runtime checkpoint references;
- model/tool usage and budget state;
- cancellation and deadline state;
- final response/artifact references or normalized error;
- created, started, updated, and completed timestamps.

Large model content, artifacts, and raw provider payloads are stored separately
with retention and sensitivity metadata.

## Step Semantics

Each nondeterministic or externally visible operation is a durable step:

- build context;
- call model;
- request approval;
- execute tool;
- wait for time/event;
- invoke/resume external runtime;
- write memory candidate;
- produce final response.

A step has an idempotency key, input fingerprint, attempt count, state, timeout,
result/error reference, and timestamps. Retrying a step reuses identity unless a
new logical operation is explicitly created.

## Public Activity Events

Clients receive concise normalized events such as:

```text
run.started
context.ready
model.started
model.output.delta
tool.requested
approval.required
tool.started
tool.completed
run.waiting
response.delta
run.completed
run.failed
run.cancelled
```

Events never expose hidden chain-of-thought. A runtime may provide a short
reasoning summary, but JARVIS classifies and stores it separately from internal
provider reasoning fields.

## Native Runtime

The native runtime should start deliberately small:

1. Receive objective and bounded context.
2. Ask a model for either a final response or typed tool intent.
3. Route tool intent through JARVIS policy/execution.
4. Append a bounded observation.
5. Repeat within turn, token, cost, time, and tool budgets.
6. Produce a final response or explicit waiting/failure state.

Planning is a strategy, not a mandatory extra model call. Simple tasks should
not pay for a complex plan.

## External Runtime Contract

An external runtime adapter must support capability discovery and a subset of:

```rust,ignore
trait AgentRuntime {
    fn descriptor(&self) -> RuntimeDescriptor;
    async fn start(&self, request: RuntimeRequest) -> RuntimeEventStream;
    async fn resume(&self, request: RuntimeResumeRequest) -> RuntimeEventStream;
    async fn cancel(&self, run: RuntimeRunId) -> Result<(), RuntimeError>;
    async fn health(&self) -> RuntimeHealth;
}
```

The wire protocol, not this illustrative trait, is the interoperability
contract. It must negotiate protocol version and capabilities, including:

- streaming and backpressure;
- tool-intent delegation versus runtime-owned tools;
- resume/checkpoint support;
- cancellation and interrupt input;
- files/artifacts;
- multimodal input/output;
- sandbox or workspace requirements;
- usage/cost reporting;
- protocol extensions.

Unsupported capabilities fail before execution, not halfway through a run.

## Runtime Tool Access

Preferred mode: the runtime emits a tool intent and JARVIS executes it. When a
runtime requires direct tool protocol access, JARVIS gives it an ephemeral,
run-scoped MCP endpoint or capability token exposing only the selected tools.
All calls still use the canonical tool pipeline.

Never pass connector credentials or a general JARVIS admin token to a runtime.

## Resume and Reconciliation

Runtime-native checkpoint IDs are opaque references. On resume:

1. Load the JARVIS run and expected state.
2. Validate runtime identity/version and checkpoint ownership.
3. Reconcile previously persisted tool calls and artifacts.
4. Mint fresh scoped capabilities.
5. Ask the runtime to resume from its checkpoint.
6. Reject duplicate completion or tool-intent events by stable event/call ID.

If a runtime claims completion but required artifacts or tool outcomes are
missing, the JARVIS run enters a reconciliation error, not `Completed`.

## Conversational Runs vs Workflows

A run is an agent interaction. A workflow is durable business orchestration that
may invoke many runs. A three-day timer or Monday schedule belongs to the
workflow engine, not an open model stream. A workflow step can call a runtime,
wait for approval, then continue independently of the original conversation.

## Safety and Limits

Every run has explicit maximums for:

- elapsed and active execution time;
- model turns, tokens, and estimated cost;
- tool calls and concurrent tools;
- context and observation bytes;
- artifact count/size;
- retries and runtime restarts;
- nested/delegated run depth.

Crossing a limit emits a typed terminal or waiting condition with an operator-
and user-actionable explanation.