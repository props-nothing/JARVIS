# JARVIS Product Specification

Status: ACCEPTED
Acceptance scope: architecture bootstrap
Version: 0.1
Last updated: 2026-09-20

## Mission

JARVIS is a persistent personal intelligence layer between a user and their
digital world. It should understand context, remember with provenance, use tools
under deterministic policy, resume long-running work, react to events, and meet
the user through text, software interfaces, or voice.

The product succeeds when a user can install it on a clean computer, connect a
model, safely grant selected capabilities, close every UI, restart the machine,
and continue interacting with the same governed system.

## Product Principles

1. **JARVIS remains the product.** Models and agent frameworks are engines.
2. **Local use is real use.** A personal installation requires no server stack.
3. **Safety is deterministic.** Models never self-authorize side effects.
4. **Memory is evidence.** Durable beliefs have provenance and correction paths.
5. **Work survives.** Approvals, timers, retries, and tasks resume after failure.
6. **Capabilities are scoped.** Discovery does not imply permission.
7. **Interfaces are replaceable.** Voice, desktop, web, and API share one core.
8. **Integrations are products.** Setup, reauth, health, diagnostics, migration,
   unload, and deletion matter as much as a successful API call.
9. **Current sources beat remembered APIs.** Every adapter starts with upstream
   evidence and a falsifiable contract test.
10. **Complexity is earned.** Add distributed infrastructure only after measured
    limits justify it.

## Primary Users

### Personal User

Wants one private assistant across laptop, desktop, phone, home, calendar, mail,
files, and voice without operating a cloud platform.

### Power User

Wants programmable tools, local models, coding runtimes, automation, auditability,
and control over where data goes.

### Small Team

Wants shared workspaces and automations while preserving per-user identity,
least privilege, approvals, and client/customer isolation.

### Integration Developer

Wants a stable SDK and test harness for capabilities without depending on the
internal agent loop or storage implementation.

## Core User Outcomes

- Ask a question and receive a streamed, traceable answer.
- Connect a provider or local model without changing the core.
- Search and act across authorized services through one tool model.
- Review and approve consequential actions before execution.
- Teach JARVIS a preference, restart it, and retrieve that memory with source.
- Schedule or trigger work that survives process and machine restarts.
- Use the same JARVIS from CLI, desktop, web, API, and phone.
- Let another trusted agent consume a narrow subset of JARVIS capabilities.
- Diagnose failures without exposing credentials or private payloads.
- Export, back up, restore, correct, and delete user data.

## Functional Requirements

### Identity and Workspaces

- `FR-ID-001`: Every product/data/capability request is associated with an
  authenticated principal. Minimal loopback liveness/readiness probes are the
  only v1 exception and expose no product data, identity, version, or capability.
- `FR-ID-002`: Every durable record containing workspace-owned data or authority
  carries workspace scope directly. Global principal identity roots may span
  workspaces but contain no workspace-private content or grants; memberships,
  sessions, credentials, policies, events, and resources bind authority/data to
  a workspace. Truly global definitions/metadata are explicitly system-scoped.
- `FR-ID-003`: Workspace scope is enforced before retrieval or execution.
- `FR-ID-004`: Local mode supports a single-owner bootstrap without pretending
  that caller-provided IDs are authentication.
- `FR-ID-005`: Server mode supports multiple users, service clients, sessions,
  scoped credentials, and revocation.

### Conversations and Runs

- `FR-RUN-001`: A conversation can contain multiple durable agent runs.
- `FR-RUN-002`: Runs expose normalized activity events and final results.
- `FR-RUN-003`: Runs support cancellation and bounded cleanup.
- `FR-RUN-004`: Durable runs can wait for approval or time and resume later.
- `FR-RUN-005`: Hidden chain-of-thought is never persisted or exposed; JARVIS
  stores concise decisions, evidence, and user-facing summaries.
- `FR-RUN-006`: Runtime-native state is isolated from canonical JARVIS state.

### Models and Runtimes

- `FR-MOD-001`: Models and model providers are separate concepts.
- `FR-MOD-002`: Routing considers capability, privacy, latency, cost, context,
  modality, availability, and explicit user policy.
- `FR-MOD-003`: Provider streaming is normalized into stable JARVIS events.
- `FR-MOD-004`: Provider retries respect idempotency and request ownership.
- `FR-RT-001`: The native runtime implements a resumable explicit state machine.
- `FR-RT-002`: External runtimes use a versioned, capability-negotiated adapter.
- `FR-RT-003`: A failed or malicious runtime cannot crash or authorize the core.

### Tools and Approvals

- `FR-TL-001`: Every capability has typed input/output schemas and stable ID.
- `FR-TL-002`: Every tool declares effects, scopes, risk, timeout, and source.
- `FR-TL-003`: Policy and approval are evaluated at invocation time.
- `FR-TL-004`: Side effects use idempotency keys and durable outcomes.
- `FR-TL-005`: Tool output is size-bounded, classified, and treated as untrusted.
- `FR-AP-001`: Approval binds principal, workspace, exact action fingerprint,
  scope, expiry, and decision.
- `FR-AP-002`: Approval channels include CLI and API first, then desktop/mobile
  and carefully authenticated voice confirmation.

### MCP and Plugins

- `FR-MCP-001`: JARVIS acts as MCP host/client for third-party servers.
- `FR-MCP-002`: JARVIS exposes selected tools as an authenticated MCP server.
- `FR-MCP-003`: Local stdio and remote Streamable HTTP are supported according
  to the negotiated official protocol version.
- `FR-MCP-004`: Protocol extensions are opt-in and capability-negotiated.
- `FR-PLG-001`: Third-party extensions are process-isolated by default.
- `FR-PLG-002`: Install manifests declare provenance, version, protocol,
  permissions, configuration schema, and compatibility.

### Memory and Context

- `FR-MEM-001`: Support working, conversational, episodic, semantic,
  preference, relationship, and procedural memory.
- `FR-MEM-002`: Durable memories include source, confidence, scope, validity,
  sensitivity, and supersession metadata.
- `FR-MEM-003`: Users can inspect, correct, forget, export, and disable memory.
- `FR-MEM-004`: Retrieval combines lexical, semantic, entity, recency,
  importance, and source-reliability signals.
- `FR-CTX-001`: Context selection is budgeted, provenance-aware, and recorded.
- `FR-CTX-002`: Policy filtering occurs before retrieval ranking and prompting.

### Events and Workflows

- `FR-EVT-001`: External and internal events use a versioned envelope.
- `FR-EVT-002`: Durable event publication uses an outbox.
- `FR-EVT-003`: Consumers declare deduplication and retry behavior.
- `FR-WF-001`: Workflows support sequence, parallelism, conditions, retries,
  timeouts, timers, event waits, approvals, cancellation, and compensation.
- `FR-WF-002`: Restart recovery is tested at every wait state.
- `FR-WF-003`: Scheduled and proactive behavior obeys notification, quiet-hour,
  budget, privacy, and escalation policy.

### Connectors

- `FR-CON-001`: Connectors have manifest, auth, lifecycle, health, diagnostics,
  migrations, and contract tests.
- `FR-CON-002`: Credentials are resolved only inside the connector boundary.
- `FR-CON-003`: Webhooks are authenticated, replay-resistant, deduplicated, and
  acknowledged independently from downstream processing.
- `FR-CON-004`: Polling and push connectors expose freshness and completeness.

### Voice and Telephony

- `FR-VOI-001`: Voice providers implement a replaceable `VoiceProvider` port.
- `FR-VOI-002`: ElevenLabs can call JARVIS through OpenAI-compatible streaming
  endpoints while JARVIS owns reasoning, memory, tools, and policy.
- `FR-VOI-003`: An ElevenLabs-owned agent can consume only explicitly exported
  JARVIS MCP tools.
- `FR-VOI-004`: Inbound calls map verified caller/session identity to scoped
  permissions; caller-provided metadata alone is insufficient.
- `FR-VOI-005`: Outbound calls require policy, consent/legal checks, budgets,
  quiet hours, reason, idempotency, status callbacks, and audit.
- `FR-VOI-006`: Interruption, end-call, transfer, voicemail, latency, and partial
  transcript behavior are explicit state transitions.
- `FR-VOI-007`: Turn boundaries are detected by a model-based end-of-turn signal
  with a configurable confidence threshold and a partial-transcript channel, not
  by a fixed silence window alone. Partial text is ephemeral observation and can
  never become a completed instruction or directly execute a tool.
- `FR-VOI-008`: Telephony carrier and audio pipeline are selected in exactly one
  place, and a text-in/text-out relay and a raw-audio mode are mutually exclusive
  per call. Provider-supplied defaults that change pipeline semantics are set
  explicitly and asserted rather than inherited.

### Installation and Operations

- `FR-OPS-001`: Signed native artifacts target supported Windows, macOS, and
  Linux architectures without requiring a developer toolchain.
- `FR-OPS-002`: Install, update, rollback, repair, backup, restore, and uninstall
  preserve or remove state according to explicit user choice.
- `FR-OPS-003`: The daemon can run foreground, as a per-user background service,
  in portable mode, or as a server container.
- `FR-OPS-004`: `jarvis doctor` produces actionable, redacted diagnostics.
- `FR-OPS-005`: A support bundle is reviewable before export.

## Non-Functional Requirements

- `NFR-SEC-001`: Deny by default at every trust boundary.
- `NFR-SEC-002`: No production secret appears in prompts, normal logs, traces,
  diagnostics, URLs where avoidable, or database columns intended for config.
- `NFR-REL-001`: Durable transitions are crash-consistent and migration-tested.
- `NFR-REL-002`: Queue, payload, recursion, concurrency, retries, and time are
  bounded and observable.
- `NFR-PORT-001`: Core behavior is tested on Windows, macOS, and Linux.
- `NFR-COMP-001`: Public protocols and persisted payloads are versioned.
- `NFR-OBS-001`: Requests, runs, workflows, model calls, and tool calls carry
  correlated identifiers.
- `NFR-PRIV-001`: Local mode can operate without cloud telemetry.
- `NFR-PRIV-002`: Retention, recording, memory, and model-data policies are
  visible and configurable.
- `NFR-UX-001`: First successful local chat requires no database or container
  installation.
- `NFR-TEST-001`: JARVIS-owned behavior is deterministically testable without
  paid providers; provider-owned behavior has gated contract/live tests.
- `NFR-VOI-001`: A voice turn's perceived latency from end of caller speech to
  first agent audio meets a recorded budget at **p50 and p95**, and is reported
  as separate components (transport/auth, end-of-turn detection, context build,
  model time to first token, tool wait, speech synthesis first audio).
- `NFR-VOI-002`: Incremental delivery is a measured model capability, not a
  boolean: a route that requires streaming is only selected for a model whose
  verified time-to-first-token and token-spread both satisfy the voice budget.

## Version 1 Scope

JARVIS v1 is an installable, single-owner local product with a path to server
mode. It includes:

- `jarvisd` and `jarvis` on Windows, macOS, and Linux;
- local authenticated daemon/client communication;
- SQLite with migrations and backup/restore;
- one OpenAI-compatible model adapter plus a deterministic fake provider;
- durable sessions and native agent runs;
- streamed text responses;
- canonical tool registry, deterministic policy, approvals, and audit;
- local safe filesystem read tools and one reference MCP connection;
- basic provenance-bearing memory and hybrid retrieval;
- durable event outbox and one resumable scheduled/approval workflow;
- health, doctor, diagnostics, signed release artifacts, and installer tests;
- API contracts that allow the desktop and ElevenLabs phases to follow without
  redesigning the core.

V1 does **not** require a desktop UI, telephone calling, every provider, every
connector, a distributed bus, Kubernetes, Redis, Temporal, or a marketplace.

## V1 End-to-End Proof

On each supported operating system, from a clean user account:

1. Install signed JARVIS binaries without Rust, Node, Python, Docker, or Postgres.
2. Complete local onboarding with a fake provider or configured real provider.
3. Start the per-user daemon and ask a streamed question from the CLI.
4. Save a sourced preference, restart the daemon, and retrieve it.
5. Request a write-like test tool, receive an approval, restart while waiting,
   approve, and complete exactly one side effect.
6. Connect an MCP inspector/client with a scoped credential and call one exported
   read-only tool while a disallowed tool remains undiscoverable.
7. Run `jarvis doctor`, create a reviewed redacted support bundle, back up state,
   restore it into a clean profile, update, and uninstall.

## Product Non-Goals

- Simulating human consciousness or claiming sentience
- Invisible surveillance or unconsented always-on cloud audio
- Circumventing provider, platform, workplace, or telephony policies
- Fully autonomous financial, legal, medical, deployment, or mass-communication
  actions without domain-specific controls
- A universal internal abstraction that erases every provider capability
- Replacing mature external agent runtimes with weaker rewrites
- Shipping a distributed platform before local correctness is proven