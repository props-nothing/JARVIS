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
call cannot be the only test of JARVIS orchestration.