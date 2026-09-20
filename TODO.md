# JARVIS Implementation Backlog

Status: ACCEPTED
Tracking state: ACTIVE
Last updated: 2026-09-20

Checkboxes describe repository state, not aspiration. An item is checked only
when its acceptance evidence exists and required validation passes.

## Status Key

- `[ ]` ready or pending
- `[~]` partial; not complete
- `[x]` complete and verified
- `BLOCKED` has a named dependency or decision

IDs are stable. Do not renumber completed work.

## Milestone 0: Specification

- [x] `DOC-001` Create root AI instructions with mandatory upstream research gate.
  Evidence: `AGENTS.md` and local Markdown-link validation.
- [x] `DOC-002` Define product mission, v1 scope, requirements, and non-goals.
  Evidence: `PRODUCT.md`.
- [x] `DOC-003` Define ordered milestones and exit gates.
  Evidence: `ROADMAP.md`.
- [x] `DOC-004` Create canonical implementation-agent prompt.
  Evidence: `MASTER_BUILD_PROMPT.md`.
- [x] `DOC-005` Create integration evidence policy and template.
  Evidence: `docs/research/integration-research-policy.md`, template, manifest,
  dependency ledger, and positive/negative validator runs.
- [x] `DOC-006` Publish architecture index and all bootstrap architecture docs.
  Evidence: `docs/architecture/README.md` indexes all owning architecture areas;
  the documentation validator requires every file.
- [x] `DOC-007` Accept bootstrap ADRs for core boundaries.
  Evidence: `docs/adr/README.md` indexes ten `ACCEPTED` ADRs.
- [x] `DOC-008` Publish runtime, tool, plugin, event, approval, local API, and
  voice-edge contracts.
  Evidence: `docs/contracts/README.md` and validator-required contract files.
- [x] `DOC-009` Publish conceptual schema and migration strategy.
  Evidence: `docs/data/schema.md`, `docs/data/migrations.md`, and retention rules.
- [x] `DOC-010` Publish threat model and abuse cases.
  Evidence: `docs/security/threat-model.md` and security documentation index.
- [x] `DOC-011` Publish testing strategy and full acceptance matrix.
  Evidence: `docs/testing/strategy.md` and the validator-counted stable scenarios
  in `docs/testing/acceptance.md`.
- [x] `DOC-012` Publish upstream repository study and official source registry.
  Evidence: `docs/research/upstream-projects.md` and `source-registry.md`.
- [x] `DOC-013` Create MCP, ElevenLabs, and Tauri evidence notes.
  Evidence: three accepted architecture notes indexed as `ARCHITECTURE_ONLY` in
  `docs/research/evidence-manifest.json`; implementation remains gated.
- [x] `DOC-014` Map every v1 requirement to milestone, contract, TODO, and test.
  Evidence: `docs/planning/traceability.md`; validator proves 69 unique rows and
  validates every row's linked owner, TODO, and acceptance IDs.
- [x] `DOC-015` Run local-link and documentation consistency checks.
  Evidence: `node --check scripts/validate-docs.mjs`,
  `node --test scripts/validate-docs.test.mjs`, and
  `node scripts/validate-docs.mjs` pass; mutation tests prove changed-path and
  empty-evidence cases fail closed.

## Owner-Controlled Public Release Gates

These items block public promotion, not local implementation or isolated CI
test-signing. An AI agent must not invent the decision or resource. See
[release readiness](docs/operations/release-readiness.md).

- [ ] `OWN-001` BLOCKED: project owner selects the license and contribution
  terms; publish the license, dependency policy, and notice rules.
- [ ] `OWN-002` BLOCKED: project owner establishes and tests a private security
  reporting channel with named response ownership.
- [ ] `OWN-003` BLOCKED: project owner and Release establish production signing
  identities, custody, rotation, backup, and compromise recovery.
- [ ] `OWN-004` BLOCKED: project owner approves the release domain, channels,
  immutable metadata location, rollback, and revocation publication path.
- [ ] `OWN-005` BLOCKED: project owner approves privacy/telemetry defaults,
  retention disclosures, and required legal review.

## Milestone 1: Foundation

Dependencies: all Milestone 0 exit criteria.

- [ ] `FND-000` Research the exact Rust toolchain, targets, crates, SQLite
  behavior, OS facilities, licenses, and versions proposed for Foundation;
  record current official evidence before dependency selection.
- [ ] `FND-001` Create Cargo workspace, pinned toolchain, workspace lint policy,
  dependency policy, and minimal crate boundaries. Depends on `FND-000`.
- [ ] `FND-002` Implement typed IDs, clock, cancellation, correlation, and domain
  error primitives with serialization tests.
- [ ] `FND-003` Implement platform config/data/cache/log/runtime path resolver with
  permissions and migration tests on Windows, macOS, and Linux.
- [ ] `FND-004` Implement layered configuration with schema, environment overrides,
  secret references, atomic writes, and version migration.
- [ ] `FND-005` Implement structured tracing, redaction, rotating local logs, and a
  test that seeded secrets never appear in logs or errors.
- [ ] `FND-006` Implement SQLite connection, migrations, migration lock, health,
  integrity check, backup, and restore.
- [ ] `FND-007` Implement `jarvisd` startup, graceful drain, health, readiness,
  single-instance lock, and authenticated local transport.
- [ ] `FND-008` Implement `jarvis status`, `config`, `service`, `logs`, and `doctor`.
- [ ] `FND-009` Implement per-user service install/remove for systemd, launchd, and
  Windows with no elevation-dependent interactive flow.
- [ ] `FND-010` Build clean-machine CI smoke tests for the exact tier-1 target
  matrix, service lifecycle, profile paths, and owner-only permissions.
- [ ] `FND-011` Build native release matrix, checksums, isolated test signatures,
  SBOM, and provenance attestations. Production signing depends on `OWN-003`.
- [ ] `FND-012` Implement install, update, rollback, portable mode, and uninstall
  tests that preserve user data unless explicitly removed. Public promotion
  depends on `OWN-001` through `OWN-005`.
- [ ] `FND-013` Implement bounded diagnostics collection and a reviewable,
  redactable support-bundle preview/export with secret-canary tests.
- [ ] `FND-014` Implement previewed and confirmed repair plans for stale locks,
  service definitions, permissions, config, and recoverable storage faults with
  backup, rollback, and verified postconditions.

## Milestone 2: Brain

Dependencies: Milestone 1 exit gate. No Brain implementation begins while a
Foundation TODO remains incomplete.

- [ ] `BRN-001` Define domain model/provider capabilities and normalized model
  stream contract.
- [ ] `BRN-002` Implement deterministic scripted model provider for tests.
- [ ] `BRN-003` Research and implement one OpenAI-compatible provider adapter.
- [ ] `BRN-004` Implement durable session/message/run/model-call repositories.
- [ ] `BRN-005` Implement native agent state machine with explicit terminal and
  waiting states.
- [ ] `BRN-006` Implement context budgeting for identity, active task, and recent
  conversation.
- [ ] `BRN-007` Implement CLI chat plus HTTP/SSE streaming.
- [ ] `BRN-008` Implement cancellation, timeout, disconnect, fallback, and daemon
  restart behavior.
- [ ] `BRN-009` Add deterministic orchestration tests and gated provider smoke test.
- [ ] `BRN-010` Implement a visible, configurable model data-use, retention,
  locality, and telemetry policy that constrains routing and records provider
  disclosures/effective decisions.

## Milestone 3: Tool Fabric

Dependencies: Milestone 2 exit gate.

- [ ] `TLS-001` Define canonical tool schema, identity, origin, effects, scopes,
  risk, timeout, and retry metadata.
- [ ] `TLS-002` Implement registry discovery independently from grants.
- [ ] `TLS-003` Implement input/output validation and bounded result storage.
- [ ] `TLS-004` Implement deterministic policy evaluation and explainable decisions.
- [ ] `TLS-005` Implement durable approval records and action fingerprinting.
- [ ] `TLS-006` Implement idempotent tool-call ledger and execution state machine.
- [ ] `TLS-007` Implement safe reference filesystem read and write-plan tools.
- [ ] `TLS-008` Refresh MCP evidence; implement stdio and Streamable HTTP client.
- [ ] `TLS-009` Implement scoped authenticated MCP server export.
- [ ] `TLS-010` Add MCP negotiation, auth, cancellation, malformed payload, and
  conformance/Inspector tests.
- [ ] `TLS-011` Define plugin manifest and process supervision contract.
- [ ] `TLS-012` Prove native/MCP/runtime routes cannot bypass policy.
- [ ] `TLS-013` Implement authenticated approval list, preview, decide, expire,
  revoke, and resume use cases for API and CLI with channel assurance checks.
- [ ] `TLS-014` Implement plugin package provenance/signature verification,
  compatibility validation, install-disabled, staged update/rollback, disable,
  data-retention choice, and removal.
- [ ] `TLS-015` Implement plugin grants, scoped launch environment, process
  supervision, resource limits, health, crash-loop quarantine, and audit.

## Milestone 4: Memory

Dependencies: Milestone 3 exit gate.

- [ ] `MEM-001` Define typed memory and provenance schema.
- [ ] `MEM-002` Implement user-confirmed preference memory lifecycle.
- [ ] `MEM-003` Implement lexical retrieval and deterministic ranking baseline.
- [ ] `MEM-004` Research and implement embedding adapter with version metadata.
- [ ] `MEM-005` Implement hybrid retrieval and query-time workspace filtering.
- [ ] `MEM-006` Implement entity candidates, confidence, merge, and split history.
- [ ] `MEM-007` Implement inspect, correct, supersede, archive, forget, export, and
  memory-disable flows.
- [ ] `MEM-008` Implement context selection ledger and sensitivity policy.
- [ ] `MEM-009` Pass restart, conflict, expiry, and cross-workspace isolation tests.
- [ ] `MEM-010` Implement explicit lifecycle and policy for working,
  conversational, episodic, semantic, preference, relationship, and procedural
  memory, including non-durable working state and confirmation rules by type.

## Milestone 5: Connectors

Dependencies: Milestone 4 exit gate.

- [ ] `CON-001` Define connector manifest, lifecycle, auth, health, diagnostics,
  and quality-scale contracts.
- [ ] `CON-002` Implement OAuth/PKCE broker and secret-reference integration.
- [ ] `CON-003` Implement webhook verification, replay defense, inbox dedupe, and
  outbox handoff.
- [ ] `CON-004` Implement pagination, incremental sync, rate-limit, and retry
  primitives.
- [ ] `CON-005` Select Google or Microsoft as first vertical after current
  official-doc research and test-account readiness review.
- [ ] `CON-006` Implement first email search/read/draft workflow.
- [ ] `CON-007` Implement first calendar search/create workflow with approval.
- [ ] `CON-008` Implement GitHub App connector and webhook ingestion.
- [ ] `CON-009` Implement generic webhook reference connector.
- [ ] `CON-010` Enforce connector quality checklist in CI.

## Milestone 6: Automation

Dependencies: Milestone 5 exit gate.

- [ ] `AUT-001` Implement versioned event envelope and durable outbox/inbox.
- [ ] `AUT-002` Implement scheduler leases, clock abstraction, and misfire policy.
- [ ] `AUT-003` Implement native workflow definitions and persisted execution.
- [ ] `AUT-004` Implement retries, timers, event waits, approvals, cancellation,
  compensation, and restart recovery.
- [ ] `AUT-005` Implement proactive rules, quiet hours, budgets, and notification
  escalation.
- [ ] `AUT-006` Build crash-point and duplicate-delivery test harness.

## Milestone 7: Runtime Ecosystem

Dependencies: Milestone 3 exit gate and `TLS-009` for scoped runtime tool
credentials where MCP is used.

- [ ] `RTM-001` Define and version runtime handshake, capabilities, request, event,
  resume, cancellation, artifact, and error schemas.
- [ ] `RTM-002` Implement isolated runtime supervisor with limits and health.
- [ ] `RTM-003` Build runtime SDK and reference fixture runtime.
- [ ] `RTM-004` Research and implement OpenClaw adapter.
- [ ] `RTM-005` Research and implement OpenAI Agents adapter.
- [ ] `RTM-006` Research and implement ACP adapter for compatible coding agents.
- [ ] `RTM-007` Research and implement LangGraph adapter where graph semantics add
  value.
- [ ] `RTM-008` Test crash, hang, protocol mismatch, malformed events, and scoped
  tool access.

## Milestone 8: Voice

Dependencies: Milestones 2, 3, 4, and 6 exit gates. Telephony live tests also
require a dedicated test account, explicit spend approval, and applicable
consent/legal policy.

- [ ] `VOI-001` Define provider-neutral voice/call contracts and latency budgets.
- [ ] `VOI-002` Refresh ElevenLabs evidence before implementation.
- [ ] `VOI-003` Implement authenticated OpenAI-compatible Responses SSE endpoint.
- [ ] `VOI-004` Implement Chat Completions compatibility only where required.
- [ ] `VOI-005` Implement ElevenLabs Custom LLM inbound voice flow.
- [ ] `VOI-006` Implement ElevenLabs scoped MCP-client mode.
- [ ] `VOI-007` Implement verified post-call webhooks and call audit records.
- [ ] `VOI-008` Implement outbound call tool with consent, quiet hours, budgets,
  idempotency, callbacks, and legal-region policy.
- [ ] `VOI-009` Test interruption, transfer, end, voicemail, disconnect, duplicate
  callback, and no-double-ring behavior.
- [ ] `VOI-010` Implement an opt-in, step-up-authenticated voice approval channel
  that is denied by default and binds the exact JARVIS approval fingerprint.
- [ ] `VOI-011` Implement the provider-neutral call state controller for latency,
  interruption/barge-in, transfer, voicemail, partial transcripts, disconnect,
  timeout, callback ordering, bounded cleanup, and terminal reconciliation.

## Milestone 9: Desktop

Dependencies: Milestone 1 exit gate for scaffolding. Each feature view depends
on its owning backend milestone; Milestone 9 cannot exit before Milestones 2
through 6 satisfy the contracts consumed by the desktop client.

- [ ] `UI-001` Research current Tauri v2 docs and refresh evidence.
- [ ] `UI-002` Scaffold Tauri/React client with generated API bindings.
- [ ] `UI-003` Implement capability-scoped IPC, CSP, and no direct secret access.
- [ ] `UI-004` Implement onboarding and daemon connection/recovery.
- [ ] `UI-005` Implement chat/activity and approval experiences.
- [ ] `UI-006` Implement memory, tasks, connections, tools, models, runtimes,
  voice, settings, and developer console.
- [ ] `UI-007` Implement signed updater and failed-update recovery.
- [ ] `UI-008` Run accessibility and desktop E2E tests on tier-1 platforms.
- [ ] `UI-009` Implement a device-bound mobile approval surface with step-up,
  notification expiry, revocation, and the same exact-action contract as CLI,
  API, and desktop.

## Milestone 10: Production Hardening

Dependencies: Milestones 1 through 9 exit gates for every feature included in
the production profile. Public release additionally requires `OWN-001` through
`OWN-005`.

- [ ] `PRD-001` Implement PostgreSQL/pgvector backend parity and migration tests.
- [ ] `PRD-002` Implement object storage and encrypted backup profiles.
- [ ] `PRD-003` Implement multi-user authentication, RBAC, service clients, quotas,
  and audit export.
- [ ] `PRD-004` Define and load-test SLOs and capacity limits.
- [ ] `PRD-005` Evaluate distributed bus/cache/workflow products from measurements;
  accept ADRs only when thresholds are exceeded.
- [ ] `PRD-006` Complete threat review, fuzzing, dependency/license review,
  penetration test, and incident exercise.
- [ ] `PRD-007` Verify data export/deletion, retention, key rotation, disaster
  recovery, rollback, and support procedures.
- [ ] `PRD-008` Implement and package the supported server-container mode with
  non-root execution, config/secret injection, migrations, readiness, drain,
  backup hooks, update, and lifecycle tests.
- [ ] `PRD-009` Run one crash-consistency, migration, compatibility, and rollback
  contract matrix across every durable aggregate and supported storage backend.

## Explicitly Deferred Until Evidence Justifies Them

- Redis
- NATS or Kafka
- Temporal as the default workflow engine
- Kubernetes
- Elasticsearch
- Qdrant or another dedicated vector database
- In-process native dynamic-library plugins
- A public plugin marketplace