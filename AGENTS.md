# JARVIS Agent Instructions

This repository builds JARVIS, a cross-platform personal AI operating system.
Treat it as durable product software, not a chatbot demo. Rust owns the control
plane. Models, agent frameworks, voice providers, databases, and external tools
are replaceable adapters.

These instructions apply to the entire repository. A nested `AGENTS.md` may add
more specific rules for its subtree but may not weaken security, verification,
or architectural boundaries defined here.

## Start Here

Before changing code or contracts:

1. Read [docs/README.md](docs/README.md).
2. Read the architecture document that owns the behavior.
3. Read every accepted ADR that constrains the change.
4. Check the current milestone and its acceptance criteria.
5. For any external product, SDK, API, protocol, or service, complete the
   integration research gate below before implementation.

Do not infer current third-party behavior from this repository, model memory,
blog posts, old examples, or the name of an API. External systems change.

## Mandatory Integration Research Gate

Before adding or changing an external integration:

1. Find the provider's official `llms.txt` or section-specific `llms.txt`.
2. Use that index to locate the exact official pages for the feature.
3. Read the official API or protocol specification, auth guide, security guide,
   limits, errors, changelog, and migration notes relevant to the change.
4. Inspect the official SDK source and examples when behavior remains unclear.
5. Check the version actually pinned by this repository. Latest docs do not
   prove compatibility with an older dependency.
6. Inspect `docs/research/evidence-manifest.json`. Architecture-only, stale,
   missing, or blocked evidence does not permit integration edits.
7. Record findings in `docs/research/integrations/<integration>.md` using
   [the evidence template](docs/research/integration-evidence-template.md).
8. Include URLs, access date, versions, supported and unsupported behavior,
   auth and secret handling, failure semantics, and a verification plan.
9. Mark uncertain claims as `UNVERIFIED`; never convert them into architecture.
10. Set the manifest entry to `IMPLEMENTATION_READY` only when the note's
  implementation gate passes for exact versions.
11. Run `node scripts/validate-docs.mjs --changed-file <path>` for every changed
  integration path before the first edit and again before completion.
12. Add a contract or live integration test that can falsify the key assumptions.

Read [the full research policy](docs/research/integration-research-policy.md).
No evidence note means no integration code. Emergency exceptions require an ADR
that names the risk, owner, and removal date.

Every new or upgraded Cargo/package dependency also requires current official
source, version, and license review. Use the compact dependency ledger only when
all of its routine-library eligibility rules pass; otherwise use a full evidence
note. An existing Foundation manifest approval is not blanket approval for later
dependency additions. Pass the package manifest/lockfile and all changed evidence
files together to `node scripts/validate-docs.mjs --changed-file <path> ...`.

### Source Priority

Use sources in this order:

1. Versioned official specification or OpenAPI/AsyncAPI schema
2. Official `llms.txt` index and official documentation
3. Official SDK source, examples, changelog, and release notes
4. Repository tests and maintainers' issue or discussion statements
5. Reputable secondary material only for context

Community examples can suggest questions. They cannot establish a contract.

## Non-Negotiable Architecture

- `jarvisd` is the long-running daemon and durable authority.
- `jarvis` is a thin CLI client and administration surface.
- Desktop, web, mobile, voice, API, and external agents are clients of the same
  core. No frontend owns business logic.
- Rust owns identity, policy, approvals, orchestration, runtime routing, model
  routing, context, canonical memory, tools, workflows, events, secrets,
  auditing, and persistence contracts.
- External agent frameworks run behind versioned runtime adapters, normally in
  separate processes. Their failure must not crash `jarvisd`.
- MCP is an interoperability boundary, not JARVIS's internal domain model.
- Every executable capability is a canonical JARVIS tool. All native, MCP,
  HTTP, and runtime-provided tools pass through the same validation, policy,
  approval, idempotency, timeout, and audit pipeline.
- The model may propose an action. Deterministic Rust policy decides whether it
  is allowed.
- JARVIS owns canonical memory. Provider sessions and runtime checkpoints are
  runtime state, not user memory.
- SQLite is the default local backend. PostgreSQL plus pgvector is the server
  backend. Core domain code depends on repository traits, not SQLx types.
- Voice is an interface. ElevenLabs and other voice providers never become the
  source of truth or bypass JARVIS authorization.
- Bind to loopback by default. Remote access requires explicit configuration,
  authentication, transport security, and scoped authorization.
- Dangerous code and shell execution never runs unsandboxed merely because a
  model requested it.

Changing any item above requires a superseding ADR.

## Dependency Direction

The intended dependency flow is:

```text
apps -> application services -> domain ports
adapters/infrastructure -------> domain ports
domain ------------------------> no infrastructure
```

The domain layer must not directly depend on Axum, SQLx, SQLite, PostgreSQL,
Tauri, OpenAI, Anthropic, ElevenLabs, LangGraph, OpenClaw, or provider SDK
types. Normalize provider payloads at adapter boundaries.

Do not split the workspace into tiny crates for aesthetics. Add a crate only
for a real ownership, compilation, security, or public API boundary.

## Implementation Workflow

1. Run `node scripts/validate-docs.mjs`; resolve documentation/evidence failures
  before selecting implementation work.
2. Start from a failing acceptance criterion, test, or narrow vertical slice.
3. State one falsifiable hypothesis and the cheapest check that could disprove
   it.
4. Read the owning code and one nearby test or caller. Avoid broad wandering.
5. Make the smallest coherent edit.
6. Immediately run the narrowest executable check.
7. Repair the same slice until that check passes before expanding scope.
8. Run broader workspace checks appropriate to the blast radius.
9. Update contracts, ADRs, source evidence, roadmap state, and operator docs in
   the same change.
10. Report what is complete, partial, unverified, and not attempted.

Never call placeholder code complete. A trait with `todo!()`, an endpoint that
returns a canned value, or a mocked integration without a real contract test is
scaffolding and must be labelled as such.

## Rust Quality Bar

Once the Rust workspace exists, the normal checks are:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Also run the narrow package or test target first. Add broader checks for
migrations, installers, generated contracts, UI, or cross-platform behavior as
the change requires.

- Use typed IDs and enums for domain concepts.
- Use structured parsers and serializers, not ad hoc string manipulation.
- Use explicit error types at library boundaries; add context at application
  boundaries.
- Never log secret values, authorization headers, raw credentials, or entire
  untrusted payloads.
- Bound queues, payloads, retries, output capture, concurrency, and timeouts.
- Pass cancellation and correlation context through every asynchronous boundary.
- Keep `main.rs` files composition-only.
- Keep public protocol types versioned and serialization-tested.
- Prefer behavior tests over snapshots of internal implementation details.

## Security Rules

Assume all of the following are untrusted:

- model output and tool arguments
- MCP metadata, descriptions, annotations, and tool output
- documents, email, websites, webhooks, and retrieved memory
- plugins and external runtimes
- caller-provided identity and workspace IDs

Therefore:

- Authenticate first, then resolve tenant/workspace server-side.
- Authorize every operation at the resource and effect level.
- Validate input and output schemas at trust boundaries.
- Treat prompt injection as data-plane input, never as policy.
- Resolve and canonicalize filesystem paths before authorization.
- Use deny-by-default network and filesystem policies for execution.
- Store secret references in normal records; resolve secret material only at the
  last responsible moment.
- Make approval records tamper-evident, scoped, expiring, and bound to the exact
  action fingerprint.
- Make side-effecting retries idempotent or explicitly non-retryable.
- Redact errors, logs, traces, diagnostics bundles, and support exports.

Security-sensitive changes require abuse-case tests, not only happy paths.

## Persistence and Durability Rules

- Persist state before publishing an event that claims the transition happened.
- Use an outbox for durable state-to-event publication.
- Define transaction boundaries explicitly.
- Every side effect has an idempotency key and recorded outcome.
- Resume from durable state, never from a reconstructed chat transcript alone.
- Version persisted payloads and test migrations from supported prior versions.
- Distinguish at-most-once, at-least-once, and effectively-once behavior in the
  contract. Do not say "exactly once" without proof.
- Approval waits, timers, retries, and restarts are first-class test scenarios.

## External Runtime Rules

- Use a versioned, capability-negotiated protocol.
- Pass scoped tool handles or capability grants, never the daemon's ambient
  credentials.
- Enforce startup, heartbeat, turn, idle, and shutdown timeouts.
- Bound stdout/stderr capture and message sizes.
- Isolate process environment variables and working directories.
- Treat runtime-reported completion as a claim; reconcile artifacts and durable
  events before marking a JARVIS run complete.
- Preserve runtime-native checkpoints separately from canonical JARVIS state.

## Tool and MCP Rules

- Canonical tool identity is stable and namespaced.
- Tool discovery never grants execution permission.
- Cache entries are scoped by server identity, auth principal, workspace, and
  negotiated protocol version.
- Revalidate ownership and authorization at invocation time, not only discovery.
- Maintain transport parity tests for stdio and Streamable HTTP where supported.
- Test initialization, negotiation, auth challenges, cancellation, progress,
  timeouts, reconnect, schema failures, and malformed output.
- External clients see an explicitly exported subset of tools. There is no
  "all tools" wildcard for remote clients in production.

## Memory and Context Rules

- Chat history is not canonical memory.
- Every durable memory has provenance, confidence, scope, sensitivity,
  validity, and correction lineage.
- Inferred memories require confidence thresholds and, for sensitive claims,
  user confirmation.
- Retrieval is hybrid and policy-filtered before ranking.
- Workspace and principal filters are applied in the query, not after retrieval.
- Context assembly records why each item was included and its token budget.
- Never persist hidden chain-of-thought. Store concise decisions, plans,
  evidence, outcomes, and user-visible reasoning summaries where appropriate.

## Integration Layout

Each first-party connector should eventually contain or reference:

```text
manifest
configuration schema
auth strategy
capability declarations
client adapter
normalization layer
webhook or polling ingress
rate-limit and retry policy
health and diagnostics
redaction rules
contract fixtures
unit and live-test plan
operator documentation
research evidence
```

Keep connector setup/unload/migration behavior explicit. A connector that
cannot be disabled, reauthenticated, diagnosed, or migrated is incomplete.

## Documentation Rules

- Architecture docs describe current intended ownership, not marketing claims.
- ADRs are immutable once accepted; supersede them with a new ADR.
- TODO checkboxes must include a stable ID and acceptance evidence.
- Mark generated files and name their generator.
- Use Mermaid for diagrams that benefit from source-controlled rendering.
- Keep local Markdown links valid.
- Record dates as ISO `YYYY-MM-DD`.
- Use exact status labels: `PROPOSED`, `ACCEPTED`, `DEPRECATED`, `SUPERSEDED`,
  `UNVERIFIED`, `BLOCKED`, `PARTIAL`, or `DONE`.

## Definition of Done

A task is done only when:

- acceptance criteria pass;
- focused and required broad tests pass;
- security and failure paths are covered;
- observability is sufficient to diagnose production failure;
- migrations and rollback are addressed when state changes;
- external assumptions have current evidence;
- docs and generated contracts are synchronized;
- no secret or unrelated file was added;
- the final report names any residual risk.

Do not commit, publish, deploy, rotate credentials, enable billing, or contact
external users unless the user explicitly requests it.