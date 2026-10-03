# Integration Evidence: Model Context Protocol

Status: ACCEPTED
Review scope: `2026-07-28` protocol and `rmcp 3.5.0` SDK, including a build verification of the selected client feature set
Owner: Tool platform
Last verified: 2026-10-03
Revalidate by: 2027-04-03
Implementation gate: PASSED

## What Changed Since 2026-09-20

The `2026-09-20` note was written from the spec landing page and the SDK
repository home. Re-reading the normative changelog and the SDK source falsified
several of its statements. The differences matter because each one describes a
method, transport, or dependency shape an adapter would otherwise be written
against.

| `2026-09-20` note said | Current official fact | Source |
| --- | --- | --- |
| `initialize` / `notifications/initialized` lifecycle | Both **removed**; every request carries protocol version and client capabilities in `_meta` (SEP-2575) | Changelog "Major changes" 1-2 |
| "Proposed version: selected during `TLS-008`" | Spec pinned at `2026-07-28`; SDK pinned at `rmcp 3.5.0` | crates.io `rmcp` 3.5.0 |
| `resources/subscribe` / `unsubscribe` + HTTP GET stream | Replaced by `subscriptions/listen`, a long-lived POST response stream with per-category opt-in (SEP-2575) | Changelog "Major changes" 4 |
| Tasks treated as a core-protocol feature | Moved out of the core into the `io.modelcontextprotocol/tasks` extension; `tasks/result` replaced by `tasks/get` polling, `tasks/list` removed (SEP-2663) | Changelog "Major changes" 6 |
| Elicitation modelled as a server-initiated request | Replaced by Multi Round-Trip Requests: the server returns `InputRequiredResult` carrying `requestState`, the client retries (SEP-2322) | Changelog "Major changes" 7-8 |
| Sampling and Elicitation listed as supported client features | Roots, Sampling, and Logging are **deprecated** by SEP-2577; new implementations should not adopt them | Deprecated-features registry |
| No mention of caching or protocol headers | `ttlMs`/`cacheScope` required on list/read results (SEP-2549); `Mcp-Method`/`Mcp-Name` required on HTTP POST (SEP-2243) | Changelog "Minor changes" |
| "legacy HTTP SSE server mode" listed as an initial JARVIS non-goal | Confirmed stronger: the legacy HTTP+SSE transport is not merely out of scope here, `rmcp` ships **no** such transport | SDK crate feature list |
| `ping`, `logging/setLevel` assumed available | Both removed; log level is per-request via `_meta` (SEP-2575) | Changelog "Major changes" 5 |

## Decision Summary

- Purpose: import third-party capabilities and export scoped JARVIS tools.
- JARVIS boundary: MCP adapter inside the governed tool fabric.
- Pinned protocol target: stable dated MCP `2026-07-28`.
- Pinned SDK: official `rmcp` `3.5.0` (published 2026-09-28; license
  `Apache-2.0`; `rust-version = 1.88`; edition 2024). Earlier releases were
  `MIT/Apache-2.0` up to `2.2.0` and `Apache-2.0` from `3.0.0`; the license text
  is recorded for the pinned version, not inherited from an older one.
- Required SDK features, client role (v1): `client`, `transport-io` (stdio),
  `transport-child-process`, `transport-streamable-http-client-reqwest`,
  `schemars`. `rmcp`'s default features are `base64`, `macros`, `server`, so the
  server half must be disabled explicitly for a client-only build.
- Deferred SDK features: `server` +
  `transport-streamable-http-server` until the scoped export (`TLS-009`);
  `auth`/`auth-client-credentials-jwt` until an authorization requirement is
  negotiated; `request-state` until MRTR sealing is needed.
- Deployment modes: local stdio and remote Streamable HTTP.
- Explicitly unsupported initially: arbitrary draft extensions; the legacy
  HTTP+SSE transport (which the SDK does not implement); Roots, Sampling, and
  Logging (deprecated by SEP-2577); MCP as canonical JARVIS domain/state.
- Kill switch: disable one server/client/export or all remote MCP.
- Gate status against the policy's [Implementation
  Gate](../integration-research-policy.md) items:
  1. evidence note — this document;
  2. exact contract version — protocol `2026-07-28` with `rmcp 3.5.0`;
  3. one falsifiable local hypothesis — `MCP-C002`: a client-only build of the
     pinned version enables the required transports;
  4. cheapest contract check — build the pinned dependency with the selected
     feature set and name each transport in a compiled signature;
  5. known unsupported behavior — legacy HTTP+SSE peers, Roots/Sampling/Logging,
     DCR as the preferred registration path;
  6. rollback/disable path — the kill switch above.

  All six items are complete. Item 4 was discharged on 2026-10-03; see
  [Build Verification](#build-verification).

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | https://modelcontextprotocol.io/llms.txt | Lists `2026-07-28`, `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05`, and `draft` | 2026-10-03 | Canonical page discovery and extension indexes |
| Latest spec landing | https://modelcontextprotocol.io/specification/latest | Resolves to `2026-07-28` | 2026-10-03 | JSON-RPC roles, stateless requests, capabilities, security principles |
| Versioned spec | https://modelcontextprotocol.io/specification/2026-07-28 | `2026-07-28` | 2026-10-03 | Normative protocol target |
| Changelog | https://modelcontextprotocol.io/specification/2026-07-28/changelog.md | `2026-07-28` vs `2025-11-25` | 2026-10-03 | **Normative list of every removal, addition, and deprecation**; the source of the "What Changed" table above |
| Deprecated registry | https://modelcontextprotocol.io/specification/2026-07-28/deprecated.md | `2026-07-28` | 2026-10-03 | Roots/Sampling/Logging, HTTP+SSE, Dynamic Client Registration, `includeContext` values, and their earliest removal |
| Versioning | https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning | `2026-07-28` | 2026-10-03 | Negotiation/compatibility and the supported-version set |
| Transports | https://modelcontextprotocol.io/specification/2026-07-28/basic/transports | `2026-07-28` | 2026-10-03 | stdio and Streamable HTTP; no session header, no SSE resumability |
| MRTR pattern | https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/mrtr | `2026-07-28` | 2026-10-03 | `InputRequiredResult`, `requestState`, `inputRequests`/`inputResponses` retry loop |
| Authorization | https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization | `2026-07-28` | 2026-10-03 | `iss` validation (RFC 9207), issuer-bound credentials, Client ID Metadata Documents |
| Security practices | https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices.md | `2026-07-28` | 2026-10-03 | Consent, tool-as-code-execution, untrusted annotations |
| Extensions index | https://modelcontextprotocol.io/docs/extensions/overview | `2026-07-28` | 2026-10-03 | Tasks, Skills, Apps, auth extensions outside the core protocol |
| Tasks extension | https://modelcontextprotocol.io/docs/extensions/tasks/overview | SEP-2663 | 2026-10-03 | `tasks/get`, `tasks/update`, `tasks/cancel`; unsolicited task handles |
| Rust SDK | https://github.com/modelcontextprotocol/rust-sdk | Current main | 2026-10-03 | Official Tokio SDK, handlers, transports, OAuth, examples |
| Rust SDK crate | https://crates.io/api/v1/crates/rmcp | `3.5.0`, published 2026-09-28 | 2026-10-03 | Exact version, license `Apache-2.0`, `rust-version 1.88`, full feature map |
| SDK source and tests | `crates/rmcp/src/**`, `crates/rmcp/tests/**` in the above repository | Current main | 2026-10-03 | Verified implementation of `server/discover`, `ClientLifecycleMode`, `subscriptions/listen`, MRTR, cache hints, standard headers |
| Inspector | https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector.md | `2026-07-28` | 2026-10-03 | Manual/CI integration inspection |

Attempted `llms.txt` URLs that did not exist:

- None for the official MCP site.

## Build Verification

`MCP-C002` was discharged on 2026-10-03 by building the pinned dependency rather
than reading its documentation. A throwaway crate in the gitignored `.scratch/`
directory declared only:

```toml
rmcp = { version = "=3.5.0", default-features = false, features = [
    "client", "transport-io", "transport-child-process",
    "transport-streamable-http-client-reqwest", "schemars",
] }
tokio = { version = "1", default-features = false, features = ["io-util", "process", "rt", "net", "time"] }
reqwest = { version = "0.13", default-features = false, features = ["rustls"] }
```

`cargo build` compiled `rmcp v3.5.0` with the `server` feature **off**, and a
signature naming each transport type compiled too, which is what proves the types
exist behind the selected features rather than merely being documented:
`AsyncRwTransport<RoleClient, _, _>`, `TokioChildProcess`,
`StreamableHttpClientTransport<reqwest::Client>`, and
`ClientLifecycleMode::Discover { preferred_versions }`.

## First Slice Shipped, and What It Changed

The implemented slices are `crates/jarvis-infrastructure/src/mcp/` — `mod.rs` normalizes what a
server *offers*, `outcome.rs` normalizes what it *returns*, and `process.rs` launches it — with 52
tests between them. The two mapping layers are pure and do no I/O, for the reason the
OpenAI-compatible adapter records: the provider-shaped traps are in the mapping, and code reachable
only through a live socket is the least tested code in an adapter. `jarvis-domain` gained no MCP
dependency.

Three findings from building it narrow the plan rather than merely annotating it:

- **The first slice is stdio-only, and the dependency is narrower than the table
  above.** It would be easy to add `transport-streamable-http-client-reqwest`
  "because the note lists it", which would silently introduce a TLS stack this
  workspace does not have. A measured stdio-only build pulls in **no HTTP client
  at all** (no `reqwest`, `hyper`, `tower-http`, `rustls`), which matches the
  `openai_compatible` adapter's existing posture of refusing a non-loopback
  endpoint. The remote transport therefore waits on a reviewed TLS decision, and
  that decision is now a **prerequisite for the remote half**, not an
  implementation detail: this note's own `auth` and Streamable HTTP rows cannot be
  honoured without it.
- **A capability namespace is not a server name.** `ToolCapability` requires the
  namespace to be a single lowercase segment, while a configured server name is a
  *dotted-or-hyphenated owner* (`brave-search`, `google.gmail`) — so the natural
  "namespace = server" reading refuses every legitimately-named server, and the
  natural repair (replace `-` with `_`) is the silent transformation the adapter
  forbids everywhere else. The namespace is a constant and the server is the
  `ToolSource` owner; two servers offering one tool name produce one capability
  and **two identities**.
- **`tools/call` has three response kinds, and only one is a result.** `2026-07-28`
  added the MRTR `input_required` response and the Tasks extension adds `task`.
  `normalize_call_response` refuses the other two **by name** rather than reading
  them as completed calls, because the alternative records a success for a call
  that has not run yet. The MRTR round loop and Task polling are control flow for
  the call path, not normalization — they need JARVIS policy and approval routing
  for the requested input, plus a bounded round count.

### Process launch, and its two hard constraints

`process.rs` prepares the child that this note's "isolated environment variables and working
directories" rule requires. What it does **not** do is sandbox: a child launched here inherits the
daemon's user, filesystem view, and network. It removes the ambient surface it *can* remove and bounds
what the child can do to the daemon. A real platform sandbox is a separate reviewed capability, and
claiming one here would make a real gap invisible.

Two constraints came out of building it, both of the kind that surface at run time rather than in
review:

- **A launched server must be dropped inside a Tokio runtime.** `rmcp` kills its child from `Drop`,
  and `kill()` awaits a reap, so dropping a transport outside a runtime **panics inside the SDK** at
  `transport/child_process.rs:50`. A daemon is async and drops in-runtime by course; the hazard is a
  synchronous shutdown path, or a test that builds a server inside a runtime and drops it after
  `block_on` returns. `McpServerProcess::shutdown` exists for the deliberate in-runtime case.
- **`env_clear()` is the whole isolation property, and the obvious way to test it does not work.**
  `Command::as_std().get_envs()` reports only the variables *explicitly set on the command* —
  inheritance happens later, inside the OS — so an assertion built on it passes identically with and
  without `env_clear()`. The test verifies the child's own printed environment instead. On Windows,
  `cmd.exe` synthesises `COMSPEC`, `PATHEXT`, and `PROMPT` for its children regardless, which the
  assertion accounts for by name.

Findings that change the adapter's shape:

- **`rmcp` requires a Tokio runtime to construct a transport.** Calling
  `TokioChildProcess::new` outside a runtime panics inside the SDK at
  `transport/worker.rs:229` with *"there is no reactor running"*. JARVIS builds
  transports from within the daemon's runtime, never from a synchronous entry
  point.
- **`rmcp`'s child-process builder defaults stderr to `Stdio::inherit()`**, which
  is why this adapter overrides it: an untrusted server's output would otherwise
  land unbounded and unredacted on the daemon's own stderr.
- **`TokioChildProcess::new` takes a `tokio::process::Command`**, not a
  `std::process::Command`; the `std` type has no `From` impl for `process_wrap`'s
  `CommandWrap` (`process-wrap 10.0.1`).
- **`AsyncRwTransport` is generic over three parameters** (`Role`, `R`, `W`), and
  the stdio transport is produced by `new_client(read, write)` over a split
  duplex stream.
- **`chrono` is in the SDK's public model surface** (`Annotated` uses
  `chrono::DateTime<Utc>`). The adapter converts to `jiff` at its boundary so
  `jarvis-domain`'s clock type does not change, matching the rule that provider
  types are normalized at adapter boundaries.
- Resolved transitive versions: `tokio 1.53.1`, `process-wrap 10.0.1`,
  `schemars 1.2.2`, `sse-stream 0.2.6`, `base64 0.23.1`, `thiserror 2.0.21`.
  (`reqwest 0.13.5` is **not** pulled in by the selected features.)
- **`rmcp` is now a workspace dependency**, pinned exactly and reviewed. The
  workspace already pinned `tokio 1.53.1`, which is precisely what `rmcp`
  resolved, so there is no version conflict. `rmcp` is intentionally absent from
  `jarvis-domain`'s dependency list.

This discharges the gate item, but the `Test Plan` below remains the acceptance
work: the build and the three layers prove the types, the mappings, and the launch,
not that a live client behaves.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| Protocol | `2026-07-28` (stateless) | `2026-07-28` | PINNED |
| SDK | `rmcp` `3.5.0` | SDK main documents 3.x | PINNED; client feature set build-verified 2026-10-03 with `server` disabled |
| Wire | stdio + Streamable HTTP | `2026-07-28` | PINNED |
| Legacy peers | `2025-11-25` and earlier via `server/discover` probe | Changelog/versioning | DOCUMENTED; `rmcp` `ClientLifecycleMode::Auto` provides the probe, conformance not yet run |

## Contract

### Authentication and Authorization

- Local stdio authority comes from explicit configured process launch plus
  JARVIS grants; the child is still untrusted.
- Remote Streamable HTTP uses the current MCP authorization family. Two
  behaviours are now normative and must be implemented rather than assumed:
  clients **MUST** validate the `iss` parameter in the authorization response
  against the recorded issuer before redeeming a code (RFC 9207), and
  persisted credentials **MUST** be keyed by issuer identifier and must not be
  reused with a different authorization server.
- Dynamic Client Registration is deprecated in favour of Client ID Metadata
  Documents. `rmcp`'s `auth` feature still supports DCR; a new adapter should
  not prefer it.
- Authenticated connection is not tool authorization. JARVIS maps every server
  and external client to workspace/source/export policy.
- OAuth client credentials is an official extension
  (`auth-client-credentials-jwt`); enable only when negotiated and required.

### Transport and Lifecycle

- Protocol is JSON-RPC 2.0 between host/client/server roles.
- **There is no session.** `2026-07-28` removed the `Mcp-Session-Id` header and
  the `initialize`/`notifications/initialized` handshake. Servers needing
  cross-call state mint explicit handles passed as ordinary tool arguments.
  JARVIS must therefore hold continuity in its own durable state rather than
  relying on a transport session, which is consistent with "resume from durable
  state" in `AGENTS.md`.
- Startup uses `server/discover`. Servers MUST implement it to advertise
  supported protocol versions, capabilities, and identity. `rmcp` exposes
  `ClientLifecycleMode::{Initialize, Discover, Auto}`; `Auto` probes with
  discover and falls back for legacy peers. Discover startup does **not** send
  `notifications/initialized`.
- Per-request metadata replaces the handshake: protocol version, client
  capabilities, and client identity travel in `_meta`
  (`io.modelcontextprotocol/protocolVersion`, `.../clientCapabilities`,
  `.../clientInfo`). A version mismatch is
  `UnsupportedProtocolVersionError`, not a hang.
- Standard transports are stdio (local) and Streamable HTTP (remote). HTTP POST
  requests **MUST** carry `Mcp-Method` and `Mcp-Name`, and tool parameters may
  carry custom headers declared with `x-mcp-header` (SEP-2243). A transport that
  omits these headers is non-conformant, not merely unusual.
- `subscriptions/listen` replaces the HTTP GET stream and
  `resources/subscribe`/`unsubscribe`. Clients opt in per category
  (`toolsListChanged`, `promptsListChanged`, `resourcesListChanged`,
  `resourceSubscriptions`) and the server tags notifications with
  `io.modelcontextprotocol/subscriptionId`. Request-scoped notifications
  (`notifications/progress`, `notifications/message`) stay on the response
  stream of the request they belong to.
- `ping`, `logging/setLevel`, and `notifications/roots/list_changed` are
  **removed**. Log level is per-request via `io.modelcontextprotocol/logLevel`
  in `_meta`, and a server must not emit `notifications/message` for a request
  that did not set it.
- SSE resumability is removed. A broken response stream loses the in-flight
  request and the client **MUST** re-issue it with a new request ID. There is no
  `Last-Event-ID` redelivery, so JARVIS cannot rely on the transport for
  at-least-once delivery and must apply its own idempotency rules.
- Every result carries a required `resultType` (`"complete"` or
  `"input_required"`); results from earlier-revision servers that omit it are
  treated as `"complete"`.
- Extensions are opt-in and declared through `extensions` on
  `ClientCapabilities`/`ServerCapabilities`.

### Data and Limits

- Tool input/output schemas use JSON Schema 2020-12; `inputSchema`/`outputSchema`
  may use any 2020-12 keyword and `structuredContent` may be any JSON value
  (SEP-2106), so a non-object schema is legal and JARVIS must not assume an
  object shape.
- `tools/list`, `prompts/list`, `resources/list`, `resources/read`, and
  `resources/templates/list` results carry `ttlMs` and `cacheScope`
  (SEP-2549). `cacheScope: "public"` explicitly permits shared intermediaries to
  cache, which is a data-classification input: JARVIS must not serve a
  `cacheScope: "public"` result from a cache scoped to one workspace, and must
  not return private data through a shared cache.
- Servers **SHOULD** return tools in a deterministic order; JARVIS should still
  compute its own canonical schema fingerprint rather than trust ordering.
- JARVIS imposes stricter message, content, tool count, concurrency, timeout,
  and artifact limits regardless of server claims.
- Tool descriptions, annotations, and content are untrusted.

### Errors and Retries

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Incompatible protocol | `UnsupportedProtocolVersionError` (-32022) on a request or `server/discover` | No | Mark incompatible, record the server's `supportedVersions`, explain supported versions |
| Header mismatch | `HeaderMismatchError` (-32020) | No | Adapter bug; fail the call and surface a diagnostic, do not retry |
| Missing client capability | `MissingRequiredClientCapability` (-32021) | No | Do not silently downgrade the request; report the unmet capability |
| Authorization required | HTTP 401 / challenge / typed SDK error | After auth flow | Do not loop; surface setup/reauth. Validate `iss` before redeeming |
| Tool schema invalid | Invalid list/call data | No | Hide tool, degrade server |
| Resource not found | `-32602` (changed from `-32002` in `2026-07-28`) | No | Classify as not-found, not as an internal error |
| Input required | `resultType: "input_required"` + `requestState` | Yes, one bounded round | Route `inputRequests` through JARVIS policy/approval, echo `requestState` unmodified, cap the round count |
| Transport closed | EOF/network close | Conditional | Per `2026-07-28` there is no resumability: re-issue the request with a new ID; reconcile side effects by idempotency key |
| Timeout/cancel | Deadline/cancellation | No automatic side-effect replay | Cancel, classify outcome, reconcile if ambiguous |
| Server unavailable | Spawn/connect failure | Bounded | Backoff, health degrade/quarantine |

## Security Analysis

- MCP metadata and output can contain prompt injection; annotations are untrusted
  unless the server is trusted.
- Tools are arbitrary code execution and require explicit consent, which JARVIS
  already enforces through its grant and approval model rather than delegating to
  MCP.
- Remote URLs require SSRF and redirect controls.
- Child processes need environment/filesystem/network/output limits.
- Discovery and cached tool lists cannot grant authority.
- Invocation rechecks source identity/schema/grant.
- `requestState` is attacker-controlled input echoed back to the server. It must
  be treated as opaque, size-bounded, and never merged into JARVIS state; the
  `request-state` feature seals it with HMAC, and JARVIS should not accept
  `requestState` that failed verification.
- External JARVIS MCP clients receive explicit allowlists, budgets, and revocable
  service credentials.
- Do not expose secrets, internal admin methods, or all-future-tools grants.

## Normalization Map

| MCP concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| Server | Tool source/plugin endpoint | Adds workspace/auth/provenance/health; `server/discover` result is the source identity |
| Tool | Canonical tool definition | JARVIS derives trusted effects/risk/scopes |
| `tools/call` | Tool call intent/execution | Always passes policy/approval/idempotency |
| `InputRequiredResult` (MRTR) | Approval/input continuation | Mapped to JARVIS policy or approval wait; `requestState` opaque; round count bounded |
| Resource | External content/resource reference | Classified, bounded, provenance retained |
| Prompt | Optional untrusted skill/template input | Never system policy automatically |
| Tasks extension | Optional external operation handle | Not canonical workflow/run state; polled via `tasks/get` |
| `subscriptions/listen` | Change-notification subscription | Drives cache invalidation only; never a durable event source |
| `ttlMs`/`cacheScope` | Cache policy hint | `cacheScope: "public"` is treated as a data-classification claim, not a licence to cross workspaces |

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `MCP-C001` | Stable spec includes stdio and Streamable HTTP, and requires `Mcp-Method`/`Mcp-Name` on HTTP POST | VERIFIED, DOCUMENTED | Versioned transports page; SEP-2243 in the changelog | A conformance run accepts a POST without the headers, or rejects either transport |
| `MCP-C002` | `rmcp 3.5.0` can serve the required client transports with the selected feature set | VERIFIED | Build verification above: `rmcp v3.5.0` compiled with `default-features = false`, and each transport type compiled in a signature | Building the pinned version with the selected features fails, or a transport is absent behind those flags |
| `MCP-C003` | The `2026-07-28` lifecycle has no session and no `initialize` handshake | VERIFIED | Changelog major change 1-2; SDK `ClientLifecycleMode::Discover` and `test_client_lifecycle_modes.rs` | The pinned SDK's discover startup still emits `notifications/initialized`, or a session header is still required |
| `MCP-C004` | `subscriptions/listen` replaces `resources/subscribe` and the GET stream | VERIFIED | Changelog major change 4; SDK `examples/clients/src/subscriptions_streamhttp.rs`, `tests/test_subscriptions*.rs` | The pinned SDK offers no subscription entry point, or still exposes only `resources/subscribe` |
| `MCP-C005` | `rmcp 3.5.0` is licensed `Apache-2.0` with `rust-version 1.88` | VERIFIED | crates.io version metadata for `3.5.0` | The published manifest for `3.5.0` states different license or MSRV terms |
| `MCP-C006` | The legacy HTTP+SSE transport is not implemented by `rmcp` | VERIFIED | Crate feature map: `transport-sse-client`/`transport-sse-client-reqwest`/`transport-sse-server` are present through `0.10.0` and absent from `0.11.0` onward | A transport for `2024-11-05` HTTP+SSE is found in the pinned `3.5.0` features |
| `MCP-C007` | JARVIS can keep MCP outside the domain model | INFERRED | Adapter architecture; normalization map above | Required semantics cannot be represented without leaking MCP types into `jarvis-domain` |

## Test Plan

### Deterministic Tests

- [ ] `server/discover` startup, including rejecting a server whose
  `supportedVersions` excludes `2026-07-28`
- [ ] Per-request `_meta` metadata is emitted on every call, not only the first
- [ ] Definition normalization and effect override
- [ ] Source/schema identity and stale cache, including `ttlMs` expiry
- [ ] `cacheScope: "public"` result is never served to or from a
  workspace-scoped cache
- [ ] Authorization/export scope
- [ ] Error mapping for `-32020`, `-32021`, `-32022`, and `-32602`
- [ ] MRTR round: `input_required` -> approval -> retry, with a bounded round
  count and an opaque, echoed `requestState`
- [ ] A tampered `requestState` is refused, not merged into JARVIS state
- [ ] Cancellation and timeout mapping
- [ ] Content limits and prompt-injection provenance

### Contract Fixtures

- [ ] Pin SDK `3.5.0` and protocol `2026-07-28`
- [ ] Official/example stdio and Streamable HTTP fixtures, including the required
  `Mcp-Method`/`Mcp-Name` headers
- [ ] A `server/discover` result fixture and a legacy server that answers the
  probe with an error (the fallback path)
- [ ] A `subscriptions/listen` fixture showing per-category opt-in and
  `subscriptionId` tagging
- [ ] A result carrying `resultType` omitted (earlier-revision server) and one
  carrying `"input_required"`
- [ ] OAuth challenge and scoped token fixture, including an `iss` mismatch that
  must be refused
- [ ] Invalid/malformed/dynamic tool fixture
- [ ] Version negotiation matrix

### Gated/Conformance Tests

- [ ] Official conformance suite for supported client/server roles
- [ ] MCP Inspector scripted smoke
- [ ] External reference server over each transport
- [ ] Reconnect/cancel/progress where negotiated, confirming that a broken
  response stream is re-issued rather than resumed

## Operational Readiness

- [ ] Per-server/client health and safe diagnostics
- [ ] Disable/revoke/quarantine/uninstall
- [ ] Protocol/SDK upgrade evidence and compatibility window
- [ ] Bounded logs/metrics/traces, with `_meta` trace context
  (`traceparent`/`tracestate`/`baggage`) propagated through existing
  observability
- [ ] Operator runbook for auth and transport failures

## Open Questions

- Resolved: the SDK is `rmcp 3.5.0` (Apache-2.0, `rust-version 1.88`).
- Resolved: no external client requires JARVIS to serve legacy SSE, because
  `rmcp` does not implement that transport; a client on `2024-11-05` HTTP+SSE
  cannot be served by this SDK at all.
- Open: whether the first slice targets only the client role, or client and
  scoped server export together.
- Open: whether MRTR sealing (`request-state`) is needed, given JARVIS treats
  `requestState` as opaque even without the HMAC seal.
- Open: which deprecated feature, if any, an existing external server still
  requires during the twelve-month window, and whether that forces
  compatibility support before the `2027-07-28` earliest removal.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-20 | Initial architecture review | Official spec/index and Rust SDK repository |
| 2026-10-03 | Specification and SDK refresh: pinned `rmcp 3.5.0` and spec `2026-07-28`; corrected the lifecycle to stateless `server/discover`; recorded `subscriptions/listen`, MRTR, caching, standard headers, and the error-code renumbering; recorded the SEP-2577 deprecation of Roots/Sampling/Logging and the removal of `ping`/`logging/setLevel`; answered the open questions on SDK version and legacy SSE | `https://modelcontextprotocol.io/specification/2026-07-28/changelog.md`, `.../deprecated.md`, `https://crates.io/api/v1/crates/rmcp`, `https://github.com/modelcontextprotocol/rust-sdk` (`crates/rmcp/CHANGELOG.md`, `tests/test_client_lifecycle_modes.rs`, `tests/test_subscriptions*.rs`) |