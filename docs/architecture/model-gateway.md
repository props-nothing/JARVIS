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

**The measurement now has a producer (`BRN-011`), and it is still an input rather than a verdict.**

Three values are recorded on every attempt that produced output: `first_output_at` (the instant the
**first** delta arrived, observed from JARVIS's own clock so it is comparable across providers),
`last_output_at` (the last delta's instant, which is what distinguishes a genuine stream from a burst
— both look identical at the start), and `output_delta_count` (the sample size, because one delta is a
burst whatever its timing). `jarvis_domain::model::capability::DeliveryMeasurement::profile` derives
the descriptor's `IncrementalDelivery` from those three, and its `observed_deltas` counts the deltas
**after the first** — so a single-delta reply is a burst by observation rather than by timing, which
is the case a first-token-only measurement cannot see.

The routing half already existed and is unchanged: `select_route` refuses a candidate whose fresh
measurement says burst, and `RouteRequirements::requires_incremental_delivery` is what makes a route
ask for it. What this closes is `BRN-043`'s remaining half — before it, `IncrementalDelivery` had
**no producer anywhere in the workspace**, so every `CapabilityDescriptor` was built with
`incremental_delivery: None`: "nobody measured this" for every model in every deployment, and no
routing decision could ever have required the capability.

Still open in `BRN-011`, and it is the half a single call cannot supply: **nothing aggregates these
per-model figures.** One attempt's measurement is not a model's delivery profile, and a descriptor's
evidence has to come from a measurement campaign against a real provider rather than from one call
this daemon happened to serve. The list item above therefore remains `UNVERIFIED` for a real model:
the recorded numbers are honest, and no campaign has yet been run against a provider that exists.

**The aggregation now exists, and the producer is connected to the consumer.**

`DeliverySamples` (`jarvis_domain::model::capability`) is a *campaign* — every measured call for one
model — and `aggregate` folds it into one `IncrementalDelivery`. Three properties make the fold a
rule rather than arithmetic:

- **The aggregate is the worst sample, not the mean.** `chunk_spread_ms` is the minimum observed,
  `observed_deltas` the minimum, and `time_to_first_token_ms` the maximum. A campaign in which any
  call burst is a model that *can* burst, so reporting the best sample — or an average that a few
  good samples lift over the threshold — would answer "this model delivers incrementally" for a model
  that sometimes does not. The field is therefore deliberately not named `average_spread_ms`.
- **`MIN_PROFILE_SAMPLES` (3) is a floor on sample size, not a confidence claim.** Two agreeing
  samples still prove nothing about the next, so a campaign below the floor attests **nothing** and
  the descriptor keeps `None` — which makes a route that requires incremental delivery refuse rather
  than pass on one observation. The floor is checked *inside* the attesting call, so a caller cannot
  reach `Some(Attested { label: Verified })` with one sample.
- **Measured evidence is `VERIFIED`, and cites no document.** `VERIFIED` is "confirmed by a live test
  against the pinned version", which is exactly what a recorded call is; `DOCUMENTED` would be wrong
  in the other direction because no document states these figures. The `source_url` is
  `jarvis://model-calls` rather than a plausible-looking `https://` address, because a fabricated URL
  cites a page that does not say what the value says — the promotion the evidence rule exists to
  prevent. The revalidation window is derived from the measurement day
  (`IsoDate::adding_days`), not written by hand, so a profile cannot be created already expired.

The read is `ModelCallRepository::delivery_campaigns`: one group per `(provider, model, revision)`
that has at least one measured call, scoped to a workspace, with the measurement filter written as
three `IS NOT NULL` clauses. **Only calls that emitted output contribute**, because a call with no
first-output instant was never measured and folding it in as zero would report a burst for a model
that was never allowed to stream.

`route_candidates` — the single builder both the run path and the daemon's diagnostic inventory call
— attests the profiles, so a probe and a run cannot disagree about a model's delivery evidence. The
daemon reads the campaigns once at startup and hands the same value to both, and
`RunPorts::delivery_campaigns` carries them into run creation.

**What is still `UNVERIFIED`, and why.** No campaign has been run against a provider that
exists, so for a real model the item above remains unmeasured: the pipeline is complete end to end
and exercised by tests, and the figures in every deployment today are the scripted provider's. A route
that *requires* incremental delivery is therefore still refused — correctly — because nothing real has
been measured. The remaining work is a campaign against a real endpoint, which is `BRN-003`'s adapter
plus operator time, not more code.

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
an actual bound rather than a value carried in a request. **The step half of that
was inert until `BRN-051`:** nothing in the product set `step_timeout_ms`, so on
every real run the `min` reduced to the run's whole remaining deadline and a provider
that accepted the connection and then stalled held the run for its full fifteen minutes.
`budget_for` now applies `DEFAULT_STEP_TIMEOUT_MS` (two minutes), which is strictly below
the deadline default so it can be the term that binds. The **token and cost
ceilings are enforced** as well: usage is captured from either arrival path (a
`usage.updated` frame or the terminal's block, whichever comes last) and checked
against the ceilings before a run may complete, so a breach fails the run and
discards its output. A ceiling with no reported usage cannot breach — refusing on
an absent value would fail every run against a provider that omits usage, so the
gap is reported through `budget_is_verifiable` rather than hidden. **Not done:**
usage is not summed across a run's calls, there is no turn/byte/concurrency budget, and the
routing consequences this section lists ("latency and cost budgets" as a routing input)
remain open — routing still does not consider a budget when selecting a route. A **retry
budget** in the sense of a spend limit is also absent, though a *retry policy* is now stated
per run and settable per request (`BRN-050`, below).

**The output ceiling's `BRN-051` half was deliberately unimplemented for one round, and the reason is
worth keeping because the obvious fix made things worse.** `RunBudget::max_output_tokens` is checked by
`exceeded_by` and nothing set the field, so the check could not fire. Defaulting the ceiling alone was
implemented and then **reverted**: the two halves of "enforce a ceiling" were missing, because the
OpenAI-compatible adapter **did not forward `limits.max_output_tokens`** to the provider — so a defaulted
ceiling would let a provider produce an over-long answer and then have JARVIS **discard** it, converting
a working answer into a failed run while constraining nothing. **A default is only correct when the
bounded component can be told to comply.**

`BRN-052` supplied the missing half. The adapter now sends the ceiling as **`max_completion_tokens`**,
which the official reference defines as "an upper bound for the number of tokens that can be generated
for a completion, including visible output tokens and reasoning tokens" — the same quantity this adapter
maps from `completion_tokens` onto `Usage::output_tokens`, and therefore the quantity `exceeded_by`
compares against. The deprecated `max_tokens` is explicitly **not** used: the reference says it "is now
deprecated in favor of `max_completion_tokens`, and is not compatible with o-series models", so sending
it would refuse exactly the reasoning models that most need an output bound. The parameter is omitted
when the run states no ceiling, because an invented limit is a limit nobody set. With both halves in
place, `budget_for` applies `DEFAULT_MAX_OUTPUT_TOKENS` (4096) and the ceiling is enforced **twice**: the
provider is told the bound, and a provider that ignores the hint is caught by the usage check.

## Auxiliary Calls and Prefix-Cache Discipline

Not every model call is a user turn. Vision, summarization, classification, and the
post-turn learning review ([ADR-0012](../adr/0012-governed-learning-loop.md)) are
**auxiliary** calls: they belong to a product feature rather than to the
conversation, and they must not be paid for as if they were a second conversation.

Three rules make auxiliary calls cheap and bounded, and all three are routing
concerns rather than prompt-string concerns:

- **An auxiliary route is named, not implied.** A feature declares the route it
  wants — a specific provider/model, or "the parent model". When it names a
  different model, that route is used; when it names none, the feature inherits the
  parent run's route. This is the same provider/model/route distinction the
  "Core Concepts" section requires, applied to a call that has no user waiting on
  it.
- **A same-route auxiliary call preserves the parent's prompt-cache prefix.** When
  the auxiliary route resolves to the parent model, the call reuses the parent's
  system prompt, tool definitions, and conversation prefix **byte-identically**, so
  a provider that caches prefixes reads a warm cache instead of re-ingesting the
  conversation. A feature that changes even the reasoning level of a same-route
  call breaks that parity, which is why parity is a property to assert rather than
  an intention to comment.
- **A different-route auxiliary call replays a digest, not the transcript.** When
  the auxiliary model differs from the parent, the cache cannot be shared, so
  replaying the full conversation would write a cold cache once per iteration for
  no benefit. The call receives recent turns verbatim plus a summary of older ones,
  which is smaller in input tokens than the transcript while preserving the recent
  context a review needs.

**Every auxiliary call carries a cumulative input-token budget.** The budget caps
the *sum* of replayed input tokens across the call's tool iterations, not the size
of any one request, because replaying the conversation on each iteration is exactly
how a "cheap" review multiplies its input cost. The loop stops before crossing the
budget, and **a call that stops on budget reports that it stopped on budget** rather
than appearing to have found nothing — a silent truncation would read as a clean
result. When no budget is configured, a conservative default is derived from the
resolved context window of the routed model, so the bound also bites on a small
local model where a fixed cloud-scale default would never apply.

Auxiliary calls are recorded as model calls under their own `task` label, so their
usage is attributable and separable from conversation usage in the audit trail.

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
metadata. **Preserved is not the same as read:** the safety flag reached the
controller and was discarded at the frame fold, so a refusal was stored as an
ordinary finish (`BRN-047`).

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

**Implemented evidence (`BRN-010`).** This list's **policy** half now has a selector:
`jarvis_domain::model::routing::select_route` takes a bounded candidate list and a
`RouteRequest` — the merged `PolicyRules`, the policy version, the content's sensitivity,
and the `RouteRequirements` — and returns the **first compliant candidate** or
`model.policy_unsatisfied`. Three properties are structural rather than documented:

- **A hard rule is checked, never relaxed.** A candidate that fails the policy or cannot
  attest a required capability is rejected *with a reason*; nothing downgrades a
  requirement because no candidate met it. There is deliberately no score, because a score
  could rank an incompliant candidate above a compliant one — this is a filter, not a
  preference.
- **Every rejection is recorded, not only the winner.** A decision naming just its
  selection could not answer "why not the local model", which is the question an operator
  actually asks.
- **Placement is read from the candidate, not guessed from the name.** An
  `EndpointClass` is required on each candidate, because this document's own point about
  provider ids applies here: `local.ollama` *looks* local and a hosted endpoint reached over
  a private network is not cloud, so a name-based guess would evaluate a local-only policy
  incorrectly. A candidate whose residency region is unknown **fails** an allow-list rather
  than passing it, and a local endpoint reports `not_applicable_local` retention rather than
  claiming a documented retention term it cannot have.

Retention and training use are **candidate-attested**, not inferred from the endpoint class,
because "this provider documents bounded retention" is a fact about its current terms that no
classification can derive. A candidate with no usable note is `provider_default` — the
provider's default terms were accepted, which documents nothing — and this document's
"Provider claims cannot be promoted into routing capabilities without current evidence" is
enforced by two separate evidence predicates, since a capability and a retention term are
established by different sources.

**Wired (`BRN-010`, completed by `BRN-014`).** The selector has callers on both the
diagnostic and the execution path. `PolicyService::evaluate` reads the **stored** policy,
constructs the `RouteRequest`, and calls `select_route_explained` for the
`GET /model-data-policy/effective` probe. `RunService::create` then does the same for a run
that will **actually execute**: it resolves the workspace's policy, selects and records a route
*before the run is created* — so a policy that admits no compliant candidate refuses the request
with `403 model.policy_unsatisfied` rather than creating a run that would have to be governed by
something — and carries the result on the run's own budget as
`RunRoute { model, decision, exception_ref }`. `RunController::resolve_model` reads that route
back and requires the provider still to serve the named model; a route naming a model the
provider no longer serves fails the run with `run.no_model_served` rather than falling back to
another model, because the policy authorized *that* one. `selected_model` survives only for the
no-policy case, which the run's budget records by carrying no route at all.

The three facts a route carries travel together because they are one decision. The **model** is
what the run may call; the **decision** is the durable record naming the considered candidates
and their rejection reasons, which the contract requires to be auditable without storing prompt
content; and the **exception** is the grant that permitted the call, when one was needed —
without it a permissive-looking route and a route a grant permitted are indistinguishable after
the fact. `model_calls.route_decision_id` records the same decision on the call row, so an
operator asking about one call gets the authorization that permitted it.

The wire model is the part that makes the selection enforceable rather than only recorded.
`ModelCallRequest` carries a **required** `model`, and the controller sends the routed one, so a
provider asked for a model it does not serve refuses with `model.provider_no_route` rather than
answering with whatever it defaults to. Without it the routed model governed *which adapter was
called* and what the `model_calls` row said, while the wire request named no model at all — so the
policy constrained the record and a provider was free to produce the text under a different model.
The stream's own `call.started` frame reports the **requested** model for the same reason: a frame
is what an operator and an audit read.

The candidate list is built by `jarvis_application::run_service::route_candidates`, which the
daemon's `ProviderInventory` also calls — one builder, so the probe and a run cannot disagree about
which models exist or where the endpoint sits.

Latency and cost budgets, current health/quota state, and task classification remain absent from
the inputs, so this list's routing inputs are only partly represented even in the selector.

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

### The policy is now reachable from a request (`BRN-048`)

The retry machinery above was, for a while, **unreachable in production**. `RetryPolicy`,
`RetryDecision`, the `FailureSite` boundary, and the retry-chain storage all existed and were
tested — but the only caller of `RunBudget::with_retry` was a test file, so every run created
through the daemon recorded `max_attempts: 1` and `RetryDecision::decide` answered `DoNotRetry`
for every failure. The tests that proved retries worked did so by constructing a budget no
request could produce. That is the same defect class as a doc comment naming a function that
does not exist, one level up: the capability was real, documented, and unreachable.

`CreateRunRequest.retry` closes it. Three design points carry the weight:

- **Absent ≠ none.** The field is optional and the wire type never defaults it, so
  `max_attempts: 1` (a caller who decided) and an omitted field (a caller who left it to
  JARVIS) stay different requests. A field defaulted to `none()` would collapse them, and the
  daemon would then be unable to tell a caller who disabled retries from one who did not care.
- **The default is bounded and named.** `RetryPolicy::default_for_run` is three attempts with a
  250ms base and a 2s ceiling; the attempt count is published on the wire as
  `DEFAULT_RETRY_MAX_ATTEMPTS` and the two are compared in `jarvis-infrastructure`, the one
  crate that depends on both. A client sizes its reconciliation window from the published
  number, so the two drifting apart would leave a client concluding a call failed while the
  daemon was still retrying it.
- **Out-of-range is refused, not clamped.** Validation happens at the trust boundary with
  `request.semantic_invalid`, because a value the daemon silently clamped is one the caller
  cannot detect and would size its own expectations against.

The retry **safety** boundary is unchanged and unaffected by this: `FailureSite` remains a
required input to the decision, the ambiguity rule is still checked first, and a request the
provider accepted is still never retried.

## The First Adapter

The first real adapter is now implemented, and where it lives is part of the
contract rather than a detail: `crates/jarvis-infrastructure/src/model_providers/`
is the manifest-gated path, so a file added there is an external-integration change
that the documentation gate will refuse until an evidence entry's
`implementation_paths` covers it. The gate is what keeps "adapter" and "evidence
note" from drifting apart.

It is one provider class, not one vendor: an OpenAI-compatible chat-completions
endpoint addressed by loopback address and port. That is a deliberately narrow
class, because the shapes an adapter must get right — framing, error codes, the
credential, the stream's terminal — are the same for every member of it.

The adapter is split so that the provider-shaped traps are testable **without a
socket**, and only the transport needs one:

| Layer | Owns | Fixture-testable |
| --- | --- | --- |
| `sse.rs` | reassembling frames from an arbitrarily chunked byte stream | yes — bytes in, frames out |
| `translate.rs` | mapping one parsed chunk onto normalized events | yes — chunk in, events out |
| `mod.rs` | socket, HTTP exchange, chunked decoding, status mapping, credential | no — hence the socket test |

That split is not stylistic. The two pure layers were already covered by fixtures
when the socket test was written, and everything it went on to find was in the
third layer: a chunked decoder that handed framing bytes to the SSE parser, so a
healthy stream died as `model.provider_malformed`. A protocol error's name on a
framing bug is exactly the failure a fixture-driven test cannot reach, because the
fixture is fed to the parser *past* the layer that was broken.

Two boundaries are enforced at construction rather than documented and trusted:

- **Plaintext is loopback-only; anything else is TLS.** The plaintext constructor refuses a
  **non-loopback host** — parsed as an address rather than prefix-matched, so a name that
  merely begins with `127.0.0.1` cannot pass. A named host is reached only through the TLS
  constructor (`tls = true`), added 2026-10-03 with its own evidence (`rustls` + `aws-lc-rs`
  over `tokio-rustls`, compiled-in `webpki-roots`; see the evidence note's "TLS transport").
  The certificate is verified against the **name**, an address is refused for TLS, and a
  failed handshake writes nothing, so the credential never reaches an unverified peer. A TLS
  endpoint is classed `ApprovedCloud`, so a local-only data policy refuses it.
- **The credential is a header, never part of a URL.** The key is a private field
  with no accessor other than the one that writes the request header, so it cannot
  be interpolated into an endpoint string, which is the shape that makes a key a
  substring of every later log line. A URL-shaped value and a key-shaped value are
  rejected on the same reasoning that `ApiKey` rejects a pasted URL elsewhere in
  this repository: a pasted URL fails upstream as a generic auth error and sends the
  operator to debug the plan instead of the value.

Provider status codes are **not** treated as JARVIS meanings. The provider's own
guide states that an exhausted balance, an organization spend limit, a project
spend limit, and a usage limit all arrive as `429` and none of them is retryable,
because retrying does not restore access; only the rate-limit family is. So the
adapter maps on the documented pair, and an unrecognized status or code becomes
`InvalidRequest` rather than a guess. The provider's raw error code is never
promoted to a JARVIS code — it is reported as evidence, so a mapped failure cannot
be mistaken for a JARVIS classification.

## Composing a Provider

An adapter that no daemon composes is a library, not a capability. The daemon now
**composes the provider once, at startup**, from the operator's own configuration
document, and a run reaches that provider or the daemon does not start.

Three rules make the composition safe rather than merely present:

- **No `[model.provider]` table means the scripted provider.** That is the default for
  a fresh install and for every test, and it is why a profile that names no endpoint
  cannot silently reach the network. The scripted provider's model identifier says
  `scripted.local`, so an operator can always tell which source served a run — which
  is a property the *identity*, not a log line, carries.
- **A configured endpoint that cannot be composed is a startup refusal.** This is the
  load-bearing rule. Falling back to the scripted provider would let a typo in a host, a
  port, or a credential route every run to a provider that answers with a fixed
  acknowledgement, and the operator would see successful runs while their real endpoint
  was never contacted. Refusing to start makes the misconfiguration impossible to miss.
- **The credential is resolved once, at composition.** The repository's rule is to
  resolve secret material at the last responsible moment; for a long-lived daemon that
  moment is startup, not each request. The value is then held for the process's lifetime,
  is never written to a record, and is never rendered — the adapter's `Debug` redacts it.
  Resolving per request would re-read the environment thousands of times and would let the
  provider's identity change underneath a run that had already selected it.

The configuration schema version moved from 1 to 2 for the added table, and **version 1
remains readable**. The version moved because a document that names a provider endpoint is
not one a version-1 binary can read: that binary's unknown-field rejection would report a
*parse* failure, when the accurate diagnostic is "written by a newer JARVIS". A file with
no provider table loads unchanged under both, so an existing profile is not rewritten
merely because the binary was upgraded.

**The schema has since moved to version 4** for the optional `tls` flag on `[model.provider]`, by the
same rule (a version-3 binary would report an unknown `tls` key as a parse failure), and **to version 3**
earlier for the `[tools]` table that ships reviewed tool
refusals — a different subsystem's table, moved by the same rule for the same reason. See
[tool-fabric.md](tool-fabric.md)'s *Refusals Can Also Be Reviewed Configuration* for what the table
means; the version arithmetic is recorded here because this section owns the configuration schema's
history, and a bump made in one place while documented in another is how the two come to disagree.

The configuration layer deliberately does **not** validate the host. Whether an endpoint is
admissible is the adapter's decision — the loopback rule above — and a second predicate in
the configuration layer could disagree with the first. So an invalid endpoint fails with the
adapter's own code, from the one place that owns the decision, and the composition carries
that code through rather than inventing a spelling for it.

The provider id and the model names **are** validated at composition, with the same
identifier rule the rest of the domain uses: a provider id reaches a `model_calls` row, a
routing decision, and a diagnostic, so "this is a legal model identity" belongs to the
domain's one definition of it rather than to an adapter's string handling.

## Credentials and Data

- Resolve provider secrets at the last responsible moment — which, for a long-lived
  daemon, is **composition at startup** rather than each request. See "Composing a
  Provider" above for why the daemon's moment is earlier than a one-shot caller's.
- Never include keys in endpoint logs, error messages, prompts, or persisted
  request bodies.
- Record secret reference ID and credential principal, not secret value.
- Classify and redact prompts, tool schemas, output, and trace metadata.
- Respect provider-specific zero-retention and residency constraints in routing.

Two shapes make the credential's exposure structural rather than a matter of care. The
configuration file holds a **reference** (`env:JARVIS_MODEL_KEY`), never a value, and the
reference parser refuses a locator that does not begin with `JARVIS_` — so a configuration
file cannot direct the daemon to read an unrelated ambient secret such as a cloud
credential or a CI token. And the adapter holds the resolved value in a private field with
no derived `Debug` and exactly one accessor, so it cannot be interpolated into an endpoint
string, which is the shape that makes a key a substring of every later log line.

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
and the two defects its negative cases found. It is also the daemon's **default**
provider, so the deterministic path is not a fixture that only tests reach.

**The composition is tested at the level it can fail.** The adapter's own socket test
proves the transport, and the composition tests prove the assembled product: a real daemon
over a real socket, pointed at a local fake OpenAI-compatible server, with a run created
through the real control API. Two things are asserted there that no provider-level test
can see — that the fake server **received** the request (so the daemon is wired to the
configured adapter and not to the scripted fallback), and that the streamed answer is
durable in the run's own events. The negative half matters just as much: an unconfigured
profile composes the scripted provider, and a configuration the adapter refuses stops the
daemon rather than falling back. Both were confirmed by mutation — replacing the
composition with the scripted provider fails the test by showing the fake server was never
called.
