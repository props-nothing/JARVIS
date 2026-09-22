# Model Gateway

Status: PROPOSED

## Purpose

The model gateway normalizes model inference without pretending all providers
are identical. It separates provider credentials/transport, model identity,
capabilities, routing policy, and normalized stream events.

## Core Concepts

- **Provider**: an API or local inference service, such as OpenAI, Anthropic,
  Gemini, Ollama, vLLM, or an OpenAI-compatible endpoint.
- **Model**: a provider-scoped model ID and optional immutable revision.
- **Runtime**: an agent loop/strategy that may use one or more models.
- **Route**: a policy decision selecting provider/model/settings for one call.

Never use those words interchangeably in contracts or configuration.

## Capability Descriptor

A model descriptor records independently verified support for:

- input/output modalities;
- streaming and terminal usage events;
- **incremental delivery as a measured property, not a flag**: verified time to
  first token *and* token spread. A model can advertise streaming and still
  deliver the whole reply in one burst, which makes a streaming pipeline a silent
  no-op: the code appears to work, the first-token number looks good, and the
  caller waits for the complete generation regardless. A descriptor that records
  `streaming: true` without a spread measurement is `UNVERIFIED` for latency
  routing purposes;
- tool/function calling and parallel calls;
- structured output and supported JSON Schema subset;
- context and output limits;
- reasoning controls and safe summary availability;
- prompt/system role semantics;
- conversation continuation or response IDs;
- caching, batch, background, and realtime modes;
- data retention/residency options;
- cost metadata and rate-limit dimensions.

Capabilities have provenance and last-verified dates. Provider marketing names
are not capability evidence.

## Normalized Request

A model call contains:

- call, run, principal, workspace, and trace IDs;
- ordered, typed input items with sensitivity labels;
- context manifest reference;
- available canonical tool schemas;
- requested output schema;
- capability requirements;
- sampling/reasoning settings where portable;
- deadline, cancellation, token, and cost budgets;
- provider-specific extension map owned by the adapter.

Provider-specific extensions are explicit and namespaced. They never leak into
the base domain request as arbitrary fields.

**Implemented evidence (`BRN-008`).** `jarvis_domain::run::budget::RunBudget` is
the typed form of the deadline, step-timeout, token, and cost budget, and
`RunBudget::call_limits()` is the single mapping from a run's budget onto the
per-call `limits` block this section requires. That mapping was missing: the
controller sent `limits.deadline: null` unconditionally, so an adapter honouring
`limits.deadline` had nothing to honour and a provider was free to wait
indefinitely. The controller now also bounds the provider `open` and each frame
wait by the tighter of the remaining deadline and the step timeout, so a budget is
an actual bound rather than a value carried in a request. The **token and cost
ceilings are enforced** as well: usage is captured from either arrival path (a
`usage.updated` frame or the terminal's block, whichever comes last) and checked
against the ceilings before a run may complete, so a breach fails the run and
discards its output. A ceiling with no reported usage cannot breach — refusing on
an absent value would fail every run against a provider that omits usage, so the
gap is reported through `budget_is_verifiable` rather than hidden. **Not done:**
usage is not summed across a run's calls, there is no turn/token/byte/concurrency/
retry budget, and the routing consequences this section lists ("latency and cost
budgets" as a routing input) remain open — routing still does not consider a
budget when selecting a route.

## Normalized Events

```text
call.started
output.text.delta
output.item.added
tool.call.delta
tool.call.completed
reasoning.summary.delta
usage.updated
provider.warning
call.completed
call.failed
call.cancelled
```

Adapters must produce exactly one terminal event and preserve provider request
IDs, finish reasons, usage, safety/refusal metadata, and retry hints in typed
metadata.

**Implemented evidence (`BRN-007`).** `jarvis_domain::model::stream::ModelStreamEvent`
is the envelope and `ModelStreamEventKind` the payload, and the list above is the
`type_name()` of every variant. Two shape defects were found by reading the contract
documents' own examples as fixtures:

- the `type` tag serialized as `snake_case` (`output_text_delta`) while the contract
  **and the type's own `type_name()`** used the dotted `output.text.delta` — two
  spellings of one event type, which would only have surfaced at the far boundary;
- the payload was `#[serde(flatten)]`ed onto the envelope, so a variant's fields sat
  beside `sequence` instead of under `payload`, where this document's normalized
  envelope and `local-control-api.md` both put them.

Both are fixed, and a test asserts the **serialized** shape as well as a parse of the
documented example, because only both together make the type and the document
interchangeable.

## Routing

Routing is deterministic application policy over a capability inventory. Inputs
include:

- required modalities, tools, schema, context, and runtime compatibility;
- workspace allow/deny lists and data-location policy;
- local-only or cloud-allowed classification;
- latency and cost budgets;
- current health, quota, and rate-limit state;
- user-selected model or preference;
- task classification and quality tier.

The route decision, considered candidates, rejected reasons, and fallback chain
are auditable without storing prompt content.

Data-use, retention, locality, telemetry, residency, sensitivity, evidence, and
exception semantics are governed by the
[model data policy contract](../contracts/model-data-policy.md). Provider claims
cannot be promoted into routing capabilities without current evidence.

## Fallback

Fallback is permitted only when the next route satisfies the same hard
capabilities and data policy. It must not silently:

- move private/local-only data to cloud;
- drop required structured output or tool semantics;
- change an explicitly pinned model;
- repeat a non-idempotent provider operation;
- exceed cost or latency budget;
- turn a provider refusal into repeated provider shopping.

The user-visible activity can state that a fallback occurred without exposing
credentials or internal provider payloads.

## Retry Ownership

Exactly one layer owns each retry. The provider adapter interprets transport
signals; the model gateway decides whether to retry the logical call.

Retry candidates:

- connection reset before an accepted/identified request;
- provider 429/5xx with bounded backoff and deadline;
- interrupted stream when provider continuation semantics are documented.

Do not retry invalid auth, insufficient scope, invalid schema, policy refusal,
content refusal, or ambiguous accepted requests unless provider idempotency is
documented and used.

**Implemented evidence (`BRN-008`).** `jarvis_domain::run::retry` holds `RetryPolicy`
and the pure decision that applies it, and the controller honours it: a transient
failure **before the provider accepted the call** closes the attempt and leaves the run
**live** so another attempt can be made, while every other failure leaves the run
then terminal. The ambiguity rule is the important half and it is enforced by
construction: `FailureSite` is a **required input** to the decision, so a policy
cannot look only at the error — and the same error must decide differently on either
side of acceptance. `decide` checks the ambiguity rule **first**, so no other rule can
reach a retry for an ambiguous request.

The retry chain is the one `BRN-004` built and nothing had used: each attempt is its
own row sharing one `logical_call_id`, numbered from 1, so "this call was tried twice"
is readable without storing prompt content. **Before this every attempt was attempt 1**
and a transient failure ended the run, so the chain's uniqueness constraint
(`logical_call_id` + `attempt`) was satisfiable but never exercised.

Three further rules: a retry must **fit the deadline**, because a backoff that outlives
it is a delay followed by the same failure (a budget-caused refusal is reported as
`run.deadline_exceeded` rather than as a provider fault, since the two send an operator
to different places); a policy that does not retry is the **default**, because retrying
spends a budget the caller never offered; and retries are **not** attempted when the
run's own consumption ceilings have been breached, since the same output would breach
again.

**Not done:** no fallback. This section's fallback list — a second route satisfying the
same capabilities and data policy — needs a capability inventory and candidate routes,
neither of which exists (`BRN-003` is the model provider, `BRN-010` the data policy).
The retry policy is also not yet settable per request: it is a field on the run's
budget, defaulting to no retry, and the `CreateRunRequest` schema has no typed override
for it.

## Credentials and Data

- Resolve provider secrets inside the adapter immediately before the request.
- Never include keys in endpoint logs, error messages, prompts, or persisted
  request bodies.
- Record secret reference ID and credential principal, not secret value.
- Classify and redact prompts, tool schemas, output, and trace metadata.
- Respect provider-specific zero-retention and residency constraints in routing.

## Testing

### Deterministic

- scripted provider stream normalization;
- split/partial UTF-8 and tool-argument deltas;
- exactly one terminal event;
- usage arriving before, with, or after output completion;
- cancellation, timeout, malformed frames, and provider error mapping;
- fallback policy and privacy constraints;
- strict structured-output validation.

### Contract

- captured official payload fixtures with provenance;
- schema/version compatibility;
- gated live call for streaming text, tool call, cancellation, and usage;
- explicit skips when credentials or account features are absent.

The first provider implementation must include the fake provider. A paid live
call cannot be the only test of JARVIS orchestration. That provider exists:
`jarvis_application::model::ScriptedProvider` replays a prepared script through
the `ModelProvider` port, so the "Deterministic" list above is assertable today —
including the negative cases (a replayed sequence, a frame for another call, an
unfinished tool call) that a well-behaved provider can never produce. See the
[model stream contract](../contracts/model-stream.md) for the rule-to-test table
and the two defects its negative cases found.