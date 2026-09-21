# Upstream Project Architecture Study

Status: ACCEPTED
Review scope: architecture bootstrap; voice framework and carrier added 2026-09-21
Reviewed: 2026-09-20

## Method and Scope

An exhaustive review of "all GitHub repositories" is neither possible nor a
sound architecture method. This review selected projects that each prove a
different hard part of JARVIS: Rust agent structure, daemon/client topology,
install/repair, protocol/tool interoperability, sandboxed runtime execution,
durable agent state, desktop packaging, connector quality, and workflows.

Only public architecture, file layout, docs, tests, and interfaces were studied.
No third-party source is copied. Licenses must be reviewed before dependency or
code reuse.

## Findings by Project

### OpenClaw: Product Operations and Extension Lifecycle

Repository: https://github.com/openclaw/openclaw

Relevant areas reviewed:

- `src/gateway/`, `src/cli/`, `src/commands/doctor*`, `src/daemon/`;
- `src/plugins/`, `extensions/`, plugin manifest/doctor tests;
- `scripts/install.sh`, release/package acceptance and E2E scenarios;
- `docs/plugins/`, `docs/gateway/`, `docs/install/`, `docs/cli/doctor.md`;
- root and nested `AGENTS.md` instruction patterns.

Adopt:

- onboarding, install, update, rollback, uninstall, health, and doctor as one
  tested product lifecycle;
- one long-lived gateway/daemon serving interfaces;
- plugin manifest, compatibility, provenance, capability consent, diagnostics,
  and package-artifact test matrices;
- generated reference docs and narrowly owned contributor instructions;
- clean-home/package acceptance tests, not source tests alone;
- separate runtime/provider/model concepts and scoped remote workers.

Do not copy:

- Node as JARVIS's required core runtime;
- OpenClaw's database/session model as canonical JARVIS memory;
- its current plugin ABI or protocol as JARVIS's internal domain model;
- its breadth before JARVIS proves a narrow vertical slice.

### Goose: Rust Agent Core, CLI/API, MCP, and ACP

Repository: https://github.com/aaif-goose/goose

Relevant areas reviewed:

- `crates/goose/` agent/session/provider core;
- `crates/goose-cli/` command surface and configuration;
- `crates/goose-mcp/` built-in MCP servers;
- `crates/goose/src/acp/` and generated ACP schema tooling;
- `ui/desktop/`, installers, manpage generation, tests, and `AGENTS.md`.

Adopt:

- Rust core shared by CLI/API/desktop surfaces;
- typed provider and MCP client boundaries;
- ACP/runtime protocol code generation and fixture tests;
- OS-aware application paths and shared configuration;
- host-controlled working directory and permission context rather than trusting
  client paths;
- central core with published SDK crates explicitly distinguished from internal
  crates.

Do not copy:

- Electron for JARVIS desktop; Tauri is selected for this product;
- provider session/memory as canonical JARVIS memory;
- a large monolithic agent module before domain/application boundaries exist.

Note: `block/goose` redirects to `aaif-goose/goose`. The docs index contained
inconsistent license wording during review; repository `LICENSE` is the source
to verify before reuse.

### OpenAI Codex: App-Server Protocol and Execution Isolation

Repository: https://github.com/openai/codex

Relevant areas reviewed:

- `codex-rs/app-server`, `app-server-client`, `app-server-protocol`, and
  `app-server-transport`;
- `codex-rs/exec-server`, sandboxed filesystem/process and remote execution;
- `codex-rs/core/src/tools`, approvals/sandbox traits, MCP runtime;
- `codex-rs/state`, rollout/session persistence, TUI/exec clients;
- integration suites for protocol, tools, permissions, Windows, and streams.

Adopt:

- a typed app-server/runtime protocol separate from UI clients;
- bounded channels, graceful drain, cancellation, process/event lifecycle;
- a distinct execution server/sandbox boundary;
- explicit output rules: protocol stdout versus diagnostics stderr;
- state runtime as preferred facade over low-level storage;
- integration tests that run real child binaries and platform behavior.

Use Codex as an external runtime. Do not merge Codex-specific permission,
rollout, provider, or execution types into JARVIS domain contracts.

### Tailscale: Daemon/CLI and Cross-Platform Operations

Repository: https://github.com/tailscale/tailscale

Relevant areas reviewed:

- `cmd/tailscaled` and `cmd/tailscale/cli`;
- `client/local`, `paths`, `health`, `feature/doctor`;
- platform service installation and updater/package code;
- dependency awareness and CLI evolution guidance.

Adopt:

- a daemon controlled through a stable local client API;
- OS-specific path and service logic behind shared abstractions;
- health as consistent subsystem state, not only an HTTP 200;
- dedicated diagnostic commands and stable JSON output modes;
- daemon launch flags only for process-global immutable concerns;
- atomic file/update behavior and dependency-budget awareness.

### Official MCP Rust SDK: Protocol at the Adapter Boundary

Repository: https://github.com/modelcontextprotocol/rust-sdk

Relevant areas reviewed:

- `crates/rmcp`, client/server handlers and feature flags;
- stdio/child process and Streamable HTTP transports;
- OAuth, version negotiation, stateless operation, examples, conformance;
- roadmap/versioning/dependency policy.

Adopt the official SDK after pinning a release and rerunning supported
conformance. Keep its types in the MCP adapter. Negotiate dated protocol versions
and opt-in extensions rather than assuming one frozen shape.

### Tauri: Desktop Security and Signed Distribution

Repositories: https://github.com/tauri-apps/tauri and plugins workspace

Relevant areas reviewed:

- capability/permission schemas and window/webview scoping;
- path APIs, bundler platform behavior, signing and updater;
- native target builds and cross-compilation warning;
- generated schemas and plugin permissions.

Adopt:

- Tauri v2 as a thin desktop client;
- explicit per-window capabilities and strict frontend/backend command surface;
- mandatory signed updates and native CI packaging;
- no direct frontend access to secrets, database, or tools.

### OpenAI Agents SDK: Runtime Lifecycle and Test Boundaries

Repository: https://github.com/openai/openai-agents-python

Relevant areas reviewed:

- `src/agents/run.py` and `run_internal/` separation;
- run state, sessions, approvals, tool/MCP guardrails;
- sandbox, realtime, voice, tracing, and testing interfaces;
- detailed `.agents/references/` ownership maps.

Adopt as an external runtime and borrow these engineering lessons:

- keep public runner entrypoint thin and lifecycle logic in named modules;
- align streaming and non-streaming behavior;
- test orchestration with scripted models, but keep transport/provider behavior
  in contract/live tests;
- serialize/resume approval and run identity carefully;
- review traces, exception chains, and hidden payloads for redaction.

### LangGraph: Checkpoints, Interrupts, and Durable Graph Semantics

Repository: https://github.com/langchain-ai/langgraph

Relevant areas reviewed:

- core `libs/langgraph`, checkpoint interfaces and SQLite/Postgres packages;
- interrupts/resume, state snapshots, retries, durability modes;
- Python SDK client boundaries for assistants/threads/runs/store/cron;
- integration tests for nested graph persistence and lifecycle.

Adopt as an external runtime for graph-shaped tasks. Map JARVIS run identity,
approvals, tools, and canonical memory explicitly. Do not expose LangGraph
checkpoint/store types as JARVIS state or make LangGraph the native core.

### Home Assistant: Integration Product Quality

Repository: https://github.com/home-assistant/core

Relevant areas reviewed:

- integration manifests and scaffold templates;
- setup/unload/migration/config-flow patterns;
- auth failure versus transient not-ready behavior;
- diagnostics and integration quality-scale validation;
- integration-owned tests and thin adapters around protocol libraries.

Adopt a JARVIS connector manifest and cumulative Bronze/Silver/Gold/Platinum
quality checklist. A connector must support setup validation, reauth, unload,
migration, diagnostics, failure recovery, and tests, not only API methods.

### Temporal: Optional Durable Workflow Backend

Official docs and repositories: https://docs.temporal.io/ and
https://github.com/temporalio

Adopt the conceptual split between deterministic workflow state and external/
nondeterministic activities. Do not require Temporal in local v1. Re-evaluate
when measured workflow scale/versioning/worker topology exceeds the native
database engine.

## Cross-Project Synthesis

### LiveKit: Realtime Voice Agent Framework and Transport

Repositories: https://github.com/livekit/agents (Python),
https://github.com/livekit/agents-js (Node), and
https://github.com/livekit/rust-sdks

Relevant areas reviewed:

- turn detection, adaptive interruption handling, backchannel distinction, and
  turn-taking tuning;
- the text-mode test framework and agent simulations;
- tool definition, async tools, tool-loop design, toolsets, and MCP client use;
- tasks, workflows, handoffs, and the agent server lifecycle (dispatch, jobs,
  graceful shutdown);
- per-stage fallback strategies for STT, LLM, and TTS;
- telephony: SIP participants, trunks, dispatch rules, connectors, DTMF, cold and
  warm transfer, and secure trunking;
- self-hosting, including the separately deployed SIP service.

Adopt as an **external runtime**, not as a core dependency, and only through the
runtime protocol:

- the testability pattern — agent behaviour asserted in text mode with no room,
  no account, and no phone call — is the finding worth borrowing regardless of
  whether the framework is ever adopted;
- the turn-handling vocabulary (end-of-turn prediction, overlapping speech, false
  interruption with resume) is a better-named model than a silence timer and
  should inform JARVIS's own turn events;
- async-tool progress so a slow tool does not leave the caller in silence;
- the telephony feature matrix as a reference for what a SIP carrier must expose.

Do not adopt:

- framework-owned tools, MCP, tasks, handoffs, or fallback as JARVIS
  implementations; each duplicates a canonical JARVIS capability (ADR-0001/0004)
  and would create the second control plane ADR-0007 forbids;
- the framework's session or checkpoint types as JARVIS state;
- a Python or Node process as the JARVIS reasoning authority.

Two constraints that decide adoption more than features do:

- **There is no Rust Agents SDK.** The framework is Python and Node, so adoption
  means a separate process behind an adapter.
- **The Rust realtime crates require libwebrtc and custom `rustflags`.** That
  conflicts with the installable-foundation exit gate's "no development runtime on
  the target machine" requirement and must be resolved explicitly before any use.

### Voice Architecture Comparison: Cascaded, Speech-to-Speech, and Framework

The three architectures were measured or reviewed for the voice milestone:

| Architecture | Shape | Measured result |
| --- | --- | --- |
| Cascaded | STT → LLM → TTS with a text-in/text-out relay | ~1306 ms perceived on real calls |
| Speech-to-speech | one model, audio in, audio out | ~1155 ms perceived on real calls |
| Agent framework | orchestration layer over either | tool calling verified headless only |

Two conclusions worth recording, because they are counterintuitive:

- The speech-to-speech advantage was **inside single-call noise** once streaming
  was added to the cascaded path, and it is bought with the loss of caller memory,
  voice selection, barge-in handling, and reconnection.
- The framework's decisive advantage is **testability**, not latency. Checking a
  tool call in a hand-rolled pipeline requires a paid call through a public
  tunnel; in a framework it is a local assertion.

Evidence: [Twilio telephony](integrations/telephony-twilio.md) and
[LiveKit](integrations/livekit.md).

## Combined Structure

The strongest combined structure is:

```text
OpenClaw/Tailscale     product lifecycle, gateway, doctor, installation
Goose                  Rust agent core, CLI/API, MCP/ACP integration
Codex                  isolated runtime and execution protocol
MCP official SDK       interoperability
Tauri                  desktop packaging and frontend capability boundary
Home Assistant         connector manifest and quality lifecycle
LangGraph/Agents SDK   optional runtime semantics and test patterns
LiveKit                optional voice framework/transport and turn-handling model
Temporal               later durable orchestration option
JARVIS                 canonical identity, memory, policy, tools, workflows, data
```

No upstream project owns all of JARVIS. The architecture deliberately combines
their proven boundaries while keeping product authority in JARVIS.