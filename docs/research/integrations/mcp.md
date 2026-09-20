# Integration Evidence: Model Context Protocol

Status: ACCEPTED
Review scope: architecture only; refresh required before implementation
Owner: Tool platform
Last verified: 2026-09-20
Revalidate by: 2026-12-19
Implementation gate: NOT READY

## Decision Summary

- Purpose: import third-party capabilities and export scoped JARVIS tools.
- JARVIS boundary: MCP adapter inside the governed tool fabric.
- Proposed version: exact dependency/spec version selected during `TLS-008`.
- Current architecture target: stable dated MCP `2026-07-28` behavior where the
  pinned official Rust SDK supports it, with negotiated older compatibility.
- Deployment modes: local stdio and remote Streamable HTTP.
- Explicitly unsupported initially: arbitrary draft extensions, legacy HTTP SSE
  server mode, MCP as canonical JARVIS domain/state.
- Kill switch: disable one server/client/export or all remote MCP.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | https://modelcontextprotocol.io/llms.txt | Lists `2026-07-28`, older, draft | 2026-09-20 | Canonical page discovery and extension indexes |
| Latest spec landing | https://modelcontextprotocol.io/specification/latest | Resolves to `2026-07-28` | 2026-09-20 | JSON-RPC roles, stateless requests, capabilities, security principles |
| Versioned spec | https://modelcontextprotocol.io/specification/2026-07-28 | `2026-07-28` | 2026-09-20 | Normative protocol target |
| Versioning | https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning | `2026-07-28` | 2026-09-20 | Negotiation/compatibility |
| Transports | https://modelcontextprotocol.io/specification/2026-07-28/basic/transports | `2026-07-28` | 2026-09-20 | stdio and Streamable HTTP |
| Authorization | https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization | `2026-07-28` | 2026-09-20 | Remote authorization family/security links |
| Security practices | https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices.md | `2026-07-28` | 2026-09-20 | Consent, access control, threat guidance |
| Rust SDK | https://github.com/modelcontextprotocol/rust-sdk | Current main | 2026-09-20 | Official Tokio SDK, handlers, transports, OAuth, examples |
| Rust SDK roadmap | https://github.com/modelcontextprotocol/rust-sdk/blob/main/ROADMAP.md | Current main | 2026-09-20 | Reports Tier 1 and stable release status |
| Inspector | https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector.md | `2026-07-28` | 2026-09-20 | Manual/CI integration inspection |

Attempted `llms.txt` URLs that did not exist:

- None for the official MCP site.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| API/spec | Exact dated version selected at implementation | `2026-07-28` | DOCUMENTED; dependency not pinned |
| SDK | Current stable `rmcp` selected at implementation | Main documents 3.x migration/current spec | UNVERIFIED until pinned |
| Wire protocol | stdio + Streamable HTTP | `2026-07-28` | DOCUMENTED |

## Contract

### Authentication and Authorization

- Local stdio authority comes from explicit configured process launch plus
  JARVIS grants; the child is still untrusted.
- Remote Streamable HTTP uses current MCP authorization standards and scoped
  client identity.
- Authenticated connection is not tool authorization. JARVIS maps every server
  and external client to workspace/source/export policy.
- OAuth client credentials is an official extension in the current docs/SDK;
  enable only when negotiated and required.

### Transport and Lifecycle

- Protocol is JSON-RPC 2.0 between host/client/server roles.
- Current stable design is stateless/self-contained per request with capability
  negotiation; compatibility behavior depends on negotiated dated version.
- Standard local transport is stdio; current HTTP transport is Streamable HTTP.
- The official Rust SDK exposes client/server features, child-process stdio,
  Streamable HTTP client/server, version negotiation, cancellation, and OAuth
  behind feature flags.
- Extensions such as tasks/skills/apps are opt-in and require both sides.

### Data and Limits

- Tool input/output schemas currently use JSON Schema 2020-12 in the reviewed
  stable material.
- JARVIS imposes stricter message, content, tool count, concurrency, timeout,
  and artifact limits regardless of server claims.
- Tool descriptions/annotations/content are untrusted.

### Errors and Retries

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Incompatible protocol | Initialize/negotiation failure | No | Mark incompatible and explain supported versions |
| Authorization required | HTTP 401/challenge/SDK typed error | After auth flow | Do not loop; surface setup/reauth |
| Tool schema invalid | Invalid list/call data | No | Hide tool, degrade server |
| Transport closed | EOF/network close | Conditional | Reconnect within budget; reconcile calls |
| Timeout/cancel | Deadline/cancellation | No automatic side-effect replay | Cancel, classify outcome, reconcile if ambiguous |
| Server unavailable | Spawn/connect failure | Bounded | Backoff, health degrade/quarantine |

## Security Analysis

- MCP metadata and output can contain prompt injection.
- Remote URLs require SSRF and redirect controls.
- Child processes need environment/filesystem/network/output limits.
- Discovery and cached tool lists cannot grant authority.
- Invocation rechecks source identity/schema/grant.
- External JARVIS MCP clients receive explicit allowlists, budgets, and revocable
  service credentials.
- Do not expose secrets, internal admin methods, or all-future-tools grants.

## Normalization Map

| MCP concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| Server | Tool source/plugin endpoint | Adds workspace/auth/provenance/health |
| Tool | Canonical tool definition | JARVIS derives trusted effects/risk/scopes |
| `tools/call` | Tool call intent/execution | Always passes policy/approval/idempotency |
| Resource | External content/resource reference | Classified, bounded, provenance retained |
| Prompt | Optional untrusted skill/template input | Never system policy automatically |
| Task extension | Optional external operation handle | Not canonical workflow/run state |
| Elicitation | Input/approval-like request | Mapped only through explicit safe UX/policy |

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `MCP-C001` | Stable spec includes stdio and Streamable HTTP | VERIFIED, DOCUMENTED | Versioned transports pages | Official conformance rejects either transport |
| `MCP-C002` | Official Rust SDK can serve client/server over required transports | DOCUMENTED | SDK README/examples/tests | Pinned release lacks required feature/conformance |
| `MCP-C003` | Version/capability negotiation can preserve older clients | DOCUMENTED | Spec/SDK versioning docs | Compatibility matrix test fails |
| `MCP-C004` | JARVIS can keep MCP outside domain model | INFERRED | Adapter architecture | Required semantics cannot be represented without leaking MCP types |

## Test Plan

### Deterministic Tests

- [ ] Definition normalization and effect override
- [ ] Source/schema identity and stale cache
- [ ] Authorization/export scope
- [ ] Error/retry/cancel mapping
- [ ] Content limits and prompt-injection provenance

### Contract Fixtures

- [ ] Pin SDK and protocol version
- [ ] Official/example stdio and Streamable HTTP fixtures
- [ ] OAuth challenge and scoped token fixture
- [ ] Invalid/malformed/dynamic tool fixture
- [ ] Version negotiation matrix

### Gated/Conformance Tests

- [ ] Official conformance suite for supported client/server roles
- [ ] MCP Inspector scripted smoke
- [ ] External reference server over each transport
- [ ] Reconnect/cancel/progress where negotiated

## Operational Readiness

- [ ] Per-server/client health and safe diagnostics
- [ ] Disable/revoke/quarantine/uninstall
- [ ] Protocol/SDK upgrade evidence and compatibility window
- [ ] Bounded logs/metrics/traces
- [ ] Operator runbook for auth and transport failures

## Open Questions

- Exact `rmcp` release and feature flags for first implementation.
- Which optional extensions, if any, are needed for v1.
- Whether any required external client still needs legacy SSE transport.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-20 | Initial architecture review | Official spec/index and Rust SDK repository |