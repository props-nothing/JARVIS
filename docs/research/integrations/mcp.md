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
server *offers*, `outcome.rs` normalizes what it *returns*, `process.rs` launches it, `client.rs`
decides which versions to negotiate and what a failed startup means, and `invocation.rs` guards a call
and its continuations — with 75 unit tests plus 5 end-to-end tests against a real fixture server in
`crates/jarvis-infrastructure/tests/mcp_conversation.rs`. The decision/mapping layers are pure and do no
I/O, for the reason the OpenAI-compatible adapter records: the provider-shaped traps are in the mapping,
and code reachable only through a live socket is the least tested code in an adapter. `jarvis-domain`
gained no MCP dependency.

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

### Startup decisions, and why two questions are not one

`client.rs` answers the two things a *connection* raises that the layers above do not: which protocol
versions to offer, and what a failed startup means. `rmcp` surfaces the latter as an eleven-variant
`ClientInitializeError` with no mapping into JARVIS terms, so without this module every startup failure
would reach an operator as the same undifferentiated string — and this note's own error table has a row
for exactly that case.

The separation that matters: a failed startup asks **may this be retried?** (`ToolErrorClass`, whose
posture is about effect duplication) and **could a retry ever succeed?** (a separate permanence flag).
Reading only the class loops forever against a server whose `supportedVersions` excludes ours, because
`Unavailable`'s posture is legitimately `Safe` — nothing ran, so nothing can be duplicated. Treating
every failure as permanent does the opposite damage, quarantining a server that was merely restarting.

Three things were delegated to the SDK after reading it rather than assumed:

- Version **selection** is the SDK's `select_protocol_version`; JARVIS's only contribution is the
  preference order in `preferred_protocol_versions`.
- The **set** of older versions to offer is derived from `ProtocolVersion::KNOWN_VERSIONS` with the
  target pinned first, because a hand-written list is a second source of truth that fails quietly: an
  SDK upgrade adding a revision would leave this function offering the old set with nothing reported.
- The session-shape question is `requires_initialize_handshake`, delegating to `has_initialize()` rather
  than comparing against the target revision. The two are not the same test — a *newer* revision than
  the target also has no handshake — and the comparison would tell a caller to wait for an exchange that
  never comes.

### Registration: where untrusted data meets the trusted claim

`registration.rs` is the join the earlier slices left open — a discovered catalog either enters the
`ToolRegistry` or is refused **by name**. Two properties make it the security-relevant step rather than
plumbing:

- **The trusted identity is a parameter, never derived.** `RegisteredTool` distinguishes them itself:
  `server` is "the configured server that supplied it. **Trusted**, unlike `definition.identity.source`".
  So `register_catalog` takes the `ServerConfigId` from JARVIS's own configuration and compares it against
  the catalog's declared owner, refusing a mismatch as `mcp.source_mismatch`. A helper that derived the
  trusted id from the catalog would agree by construction and be **unfalsifiable** — the mutation that
  disables the check fails three tests, one of which asserts that an impostor does not poison the
  registry's own **first-wins** source claim (and thereby lock the real server out permanently).
- **A *replacement* is unrepresentable.** Every registration uses `RegistrationRequest::new`, which refuses
  a change to a known identity; `replacing` is never called. That matters because a replacement keeps the
  identity, so "every approval recorded against it still matches, while the implementation behind it is
  new" — `ACC-024` verbatim. There is deliberately no flag to widen this: a re-registration after a
  reviewed change is a different operation with its own review.

One finding came from a test premise I got wrong. I asserted a *changed schema* is refused at registration;
it is not, and correctly so — a schema change moves the fingerprint, so it moves the **identity**, and only
a same-identity-different-content change is refused. The real consequence is that one capability can hold
**two identities**, which the registry permits deliberately. `RegistryCatalog` is keyed by **capability
string**, so building one from such definitions would silently drop a tool, and a call resolving it would
report `tool.not_found` for a registered tool. The publish join therefore **refuses** a duplicate capability
rather than producing a lossy catalog.

### Publishing reads the registry, not the catalog — and the difference is a security property

The first version of the publish join (`catalog_pairs`) converted the **offered** catalog into
`(ToolDefinition, Option<String>)` pairs. That is a defect, not a simplification: the catalog is what the
server *offered* while the registry is what was **admitted**, and the two sets differ exactly where the
refusals are. Publishing the offered set would serve tools `register_catalog` had just refused:

- A tool refused as `IdentityChanged` is `ACC-024` — its schema moved under an identity approvals are
  recorded against. Publishing it dispatches an implementation no approval was ever matched to.
- A catalog refused as `SourceMismatch` is an **impersonation**. Publishing it dispatches that server's
  tools while the only record saying it was refused sits beside a catalog serving it.

`publishable_pairs(registry, server, catalog)` therefore filters by **identity *and* attribution**.
The identity half covers a changed definition (a different identity, simply absent); the attribution half
(`registered.server == server`) covers what identity alone cannot — a second server declaring the first's
source and producing a byte-identical definition resolves to the *first* server's registration, and it was
refused as `SourceClaimed`.

The filtering is not the silent drop a filter usually is: every excluded tool was already reported in the
`RegistrationReport` returned alongside, with its capability and reason, so a caller that ignores the report
gets a *smaller* catalog — the fail-closed direction.

Three mutations falsify this, each in its own test: removing the admission filter entirely; dropping the
attribution half; and checking the capability collision over the admitted subset rather than the offered
catalog.

⚠ **The third of those found a wrong fixture rather than a missing guard.** My first collision test
registered *both* identities and then claimed an admitted-subset check would pass — it would not, because
both were admitted and the filter changed nothing. The mutation was caught by the *older* collision test
instead. The fixture now registers one identity and offers two, so the subset check genuinely sees one tool;
re-running the mutation then killed the corrected test as well. **A test whose name names a state its fixture
does not create proves nothing about that state.**

### The catalog carries its schemas, and the reason is one layer downstream

`NormalizedCatalog` returns `schemas: Vec<String>` alongside `tools`. This closes a defect that no test in
this adapter could have caught, because it manifests in the *next* layer: `normalize_tool` parsed each
`inputSchema` to fingerprint it and then **discarded the text**, and a `ResolvedTool` whose
`input_schema` is `None` is refused by the argument validator as `tool.schema_absent`. So a tool this
adapter had just accepted would register and then be **uncallable** — the "a value nothing consults"
shape, one layer along. The finding came from reading `RegistryCatalog`, which is constructed from exactly
`(ToolDefinition, Option<String>)` pairs, and noticing the adapter produced no such pair.

Three properties, each asserted:

- **Alignment by construction.** Definitions and schemas are sorted **as pairs** and then unzipped, so
  they cannot drift apart. A test with **distinct** schemas pins the association — the first version used
  one shared schema, which made a reversal unobservable and let the mis-alignment mutant **survive**.
- **The carried text is the validated one** (`ToolSchema::text`), so it is byte-identical to what the
  identity's fingerprint was computed over. `ToolSchema::confirms` is what a caller storing it will be
  checked against, and that is asserted end to end against a real server.
- **Key order does not change the identity.** `serde_json::Map` is a `BTreeMap`, so a schema is
  re-serialized with sorted keys before fingerprinting. That is a property rather than a nuisance: a
  server reordering its schema's keys does not present a new tool (which would invalidate every grant for
  it), and two spellings of one schema cannot look like two tools. `a_reordered_schema_presents_the_same_identity`
  asserts it directly.

### A real conversation, and the bug that made one necessary

`crates/jarvis-infrastructure/tests/mcp_conversation.rs` runs JARVIS's client against a **hand-written
fixture `ServerHandler` over a real duplex pipe** — five tests that no unit test can replace, because
discovery is a lifecycle an in-memory fixture cannot exercise. `rmcp`'s `server` feature is enabled for
**dev-dependencies only**, so the shipped binary stays client-only (`client`, `transport-io`,
`transport-child-process`) while the test build can run a genuine peer.

What they prove: discovery really reaches a server and reports its identity and versions; a tool the
server *actually listed* normalizes to the same canonical values the in-memory fixtures assert; a call's
content normalizes to a bounded body; a `METHOD_NOT_FOUND` a real peer returns classifies as `NotFound`
rather than `ProviderError`; and a peer that shares no version fails as a **permanent** startup failure.

**The fixture's first version was wrong, and the failure mode is worth recording.** It ran the server as
`let _ = serve_server(..).await` — which does not hold the `RunningService`, it **drops it immediately**,
and its `Drop` cancels the connection. Discovery still passed, because it is the first message and won the
race; every later call failed `TransportClosed`. The fix is to await `service.waiting()`, which both keeps
the service alive and ends the task when the connection does. Confirmed by mutation: re-introducing
`let _ = service.waiting()` fails exactly the three tests that exercise the conversation *after* the
handshake, while discovery keeps passing — which is precisely the signature of the original bug.

### The multi-round-trip refusal is a decision, not a gap

`invocation.rs` implements the sentence `outcome.rs` refused with ("it needs JARVIS policy and approval
routing for the requested input, and a bounded round count") — and building it produced the finding that
makes the refusal correct rather than merely unimplemented.

**Every MRTR input request is a capability JARVIS must not lend out.** `InputRequest` has exactly three
variants, and each is a feature this note already records as unsupported:

| Variant | What it asks the *client* to do | Note's existing status |
| --- | --- | --- |
| `CreateMessage` (sampling) | Run a model call, return the completion | Deprecated, SEP-2577 |
| `ListRoots` (roots) | Return the client's directory list | Deprecated, SEP-2577 |
| `Elicitation` | Prompt the client's user for input | Replaced by this very pattern |

None is a data interchange: each asks JARVIS to spend something it owns on a server's behalf — **model
budget**, the **shape of the filesystem**, or the **user's attention and consent**. `ListRoots` in
particular is a remote server enumerating the workspace layout, which is an information-disclosure
primitive dressed as a convenience. So the default is *refuse by name*, and a future version servicing
one would be adding a capability with its own research gate rather than filling in a stub. The note's
"Explicitly unsupported" list is therefore **stronger than it claimed**: these are not merely unadopted
features, they are the exact set of things an MRTR continuation can ask for.

Two further decisions in that module:

- **The tool invoked must be the tool authority was recorded against**, delegated to
  `ToolIdentity::authorizes` rather than compared here — discovery is untrusted and a server can re-schema
  a tool between the listing and the call, so the check is made at invocation time. The mutant that
  compares only the capability is killed by two tests, one of them the `ACC-024` schema case.
- **The round bound is a compile-time assertion, not a test.** JARVIS's bound must sit below the SDK's
  default so JARVIS's refusal is the one an operator sees; both sides are constants, so clippy rejects a
  runtime comparison as "this assertion has a constant value" — correctly, since it cannot fail at run
  time. Checking it also corrected a wrong belief: the SDK default is **10**, not 3.

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

### Discovery: the caller, and the three assumptions it corrected

`discovery.rs` is the piece the slices above left open — it spawns a local server, drives the discovery
handshake, lists its tools, and returns the normalized catalog beside a still-live client. It is exercised
against a **real child process** (`tests/mcp_discovery_process.rs` spawning `examples/mcp_fixture_server.rs`),
because the spawn is exactly what a duplex-pipe fixture cannot falsify. An example rather than a `[[bin]]`:
the `server` feature is a *dev*-dependency, so only an example can both exist and speak the server protocol
(and the harness is off, since libtest's stdout banner would interleave with the JSON-RPC framing).

Writing it disproved three things the note had implied:

- **An older peer is discovered, not refused.** `Discover` resolves a version from the intersection of
  `preferred_versions` and the peer's set, so a peer reporting **only** `2025-06-18` is a legitimate session
  at that version — the same lifecycle the note records as lacking per-request `_meta`. There is no floor.
  The version is therefore **recorded** (`DiscoveredServer::protocol_version`) instead of promised, and a
  test named for the rule asserts the downward negotiation.
- **"The server offers nothing" and "the server offers nothing usable" are different facts**, and both
  arrive as an empty `tools` vector. The second is `mcp.no_usable_tools`, carries the first rejection so the
  message names the field at fault, and is permanent.
- **The identity cannot come from the launch spec.** `McpLaunchSpec` holds a program, arguments, and an
  environment — no name. A path-derived name is not a usable owner, and slugging the path is the silent
  transformation that makes two servers' tools share an owner. The configured name is a parameter.

Two further properties, each falsified by mutation rather than asserted by inspection:

| Property | Why it is load-bearing | Mutation that fails it |
| --- | --- | --- |
| Both name rules are checked, **before** the spawn | An identity-valid name can be an owner-invalid one; the symptom would be an all-refused catalog for a name-shaped cause | Delete the owner check → `a_name_that_is_a_valid_identity_but_not_a_valid_owner_is_refused_up_front`; move both after the spawn → `an_unusable_configured_name_is_refused_before_any_process_is_spawned` reports `Launch { code: "mcp.spawn_failed" }` |
| A failure carries the child's stderr | `into_transport` consumes the process, so the diagnostics would be dropped with it and a failed server would explain nothing | Empty the refusal's `diagnostics` → `a_child_that_is_not_a_server_fails_as_a_startup_refusal_carrying_its_stderr` |

One documented behavior is recorded rather than worked around: **`Peer::list_tools` consults a
per-connection response cache** before the wire, honouring the server's `ttlMs`/`cacheScope`. A discovery
reflects the current listing or a cached one within that TTL; because the cache is per-connection and a
discovery's connection is new, only a *re*-listing on a live connection could be served from it.

### The call path: one derived name, one repeated guard, and a wiring test that had to be reordered

`call.rs` gives `invocation.rs` and `outcome.rs` their first production caller — until it existed, both
modules' decisions were tested and **unreached**. Four pure functions, so each decision is testable without
a server:

| Function | Decision |
| --- | --- |
| `wire_name` | canonical `mcp.<name>@1` → the server's own `<name>`, or a refusal |
| `call_params` | the `tools/call` request, refusing a non-object argument document |
| `decide_response` | complete, or refused — with the identity guard on **every** response |
| `normalize_response` | a body, or the class a reported failure carries |

**The name is derived, not guessed, and the refusal is two-sided.** The mapping reverses
`capability_for` through the domain's own accessors, and refuses a foreign **namespace** *and* a different
**major** — so an implementation that stripped a prefix from any capability cannot pass. Dropping only the
namespace check fails its own test; always refusing kills four others.

**The identity guard repeats per response, on purpose.** A multi round-trip continuation is a fresh request
against the same identity, so a guard run once at the start leaves every later round unchecked — and a
server can re-schema a tool between the listing and the call. This is `ACC-024` at invocation time.

**The transport classes stay in `outcome.rs`.** `TransportSend` → `Unavailable` (nothing ran) is a different
row from `TransportClosed` → `ProviderError` (may have executed; unsettled). A lost response reported as
`Unavailable` tells a caller a write can be safely repeated when it may already have happened, so this
module neither restates nor re-exports the mapping. Three one-line delegations authored first were deleted
for the same reason: **a second spelling of one fact is this workspace's most-repeated defect.**

⚠ **The wiring test was wrong on the first attempt, and the reordering is the finding.** Sending the
canonical string instead of the wire name is the mutant that matters, and the first end-to-end version
asserted `params.name == TOOL_NAME` *before* calling — so the mutant died on the test's own assertion and the
exchange was never exercised. With the call made first and required to **succeed**, the fixture — which
refuses any name it did not list — answers `McpError(ErrorCode(-32601), "no such tool")` under the same
mutation. **A wiring test must let the peer detect the fault; an assertion placed before the call makes it a
second copy of the unit test.**

### The executor: the MRTR loop is the SDK's, and JARVIS decides which rounds to stop on

`McpToolExecutor` closes the last gap between the decisions above and the `ToolExecutor` port. It holds a live
session plus the identities **registration admitted**, and drives `tools/call` through `execute`. Two
corrections came out of building it, and neither was visible from documentation:

**⚠ The MRTR loop is not on `Peer`.** `Peer<RoleClient>` owns the single-round-trip senders
(`call_tool_once`); `call_tool_with_mrtr_max_rounds` is implemented on `RunningService<RoleClient, S>`,
because it drives several round trips and needs the client handler to fulfil the intermediate requests.
Holding only a cloned peer — my first version — would have meant reimplementing the loop, which is precisely
the second implementation this module exists to avoid. **So the executor holds the session behind an `Arc`,**
and JARVIS supplies only the *round cap*: `MAX_MRTR_ROUNDS`, asserted below the SDK's own default at compile
time, so an operator reading `mcp.round_limit_exceeded` learns JARVIS's bound rather than the SDK's.

**⚠ A fixture that lists a tool must be able to serve it.** `examples/mcp_fixture_server.rs` implemented
`list_tools` but not `call_tool`, so the SDK's **default handler** answered every call with
`METHOD_NOT_FOUND "no such tool"` — for a tool the fixture had just listed. The executor returned `NotFound`,
which reads exactly like a wrong-name bug and is not one. It was diagnosed by printing the name actually sent
(`read_file`, correct) and then checking which handlers the fixture implemented. The fixture now serves its
tool and keeps the refusal for a name it did not list, which is what lets the executor suite rely on the peer
to detect a wrong name.

**The safety-relevant mapping is `Ambiguous`.** `Timeout` and `ProviderError` mean an effect **may exist**;
the port separates `Ambiguous` from `Failed` because the recovery pass treats them differently, and reporting
an unsettled class as a plain failure tells it a possibly-executed call did nothing — the duplicate-effect
direction this note's retry table exists to prevent. The mapping lives inside `execute`, so no unit test can
reach it: the fixture gained an **`unanswered` mode** (it accepts the call and withholds the answer for 30 s)
and the test uses a 250 ms bound, making the timeout deterministic rather than a race.

Three mutations falsify the executor, each in the test named for it: dropping the `Ambiguous` arm
(`left: Failed(Timeout)`), skipping the membership check, and dropping the cancellation race.

One consolidation: `DiscoveryClient` and `CallClient` were two empty declarations of one posture in two
modules, and are now a single `JarvisClient`.

### Routing: the pipeline takes one executor, so the choice has to be a value

`ToolCallService::new` holds a single `Arc<dyn ToolExecutor>`, and the composition passes native. That is a
deliberate shape in the port — a *list* of executors would make dispatch order invisible — but it means a
second source of tools has nowhere to go. `RoutingExecutor` fills the one slot and routes by the tool's
**source kind**, and is registered in the daemon's tool fabric by `daemon::router_over`, which is the adapter's
first production caller.

**The key is the kind, not the capability namespace.** A namespace is a contract's rather than a transport's
— this adapter deliberately reuses one constant for every server — and it is not trusted the way a source is:
by the time a call reaches an executor the identity has been authorized and `ToolIdentity::authorizes`
compared the source. A tool named `mcp.something` from a native build would route to a server that never
offered it if the namespace decided.

**An unrouted kind is `NotFound`, never a fallback.** The fallback is what a hurried version writes ("native
is the common case") and its failure is a *misroute*: a server's tool name in front of the daemon's own clock
or filesystem.

Only `Native` is registered, because it is the only kind the daemon can currently implement — an MCP executor
needs a live session and a server declaration, and neither exists in configuration yet. **So the named
remaining gap is an MCP server configuration section**: without a way for an operator to declare a server,
`discover_stdio_server` has no input and the `McpServer` kind cannot be registered.

⚠ **The first misroute test did not test the misroute.** It routed a `Native` tool and asserted native ran —
which a fallback implementation does identically, because that kind *is* routed. The mutation was caught only
by the unrouted-refusal test, revealing that the name claimed a state its fixture never created. Rewritten to
register only native and attempt an MCP call; re-running the mutant then failed both tests. **Re-run a
mutation after fixing its fixture, or the fix is an assumption.**

### The declaration: a server's environment is references, never literals

That remaining gap is now closed by `config/mcp.rs` — an optional `[mcp]` table whose entries declare a name,
a **resolved** program, arguments, an environment, and a startup timeout.

**The environment holds `SecretReference` values, and that is strictly stronger than the launcher's own
literal pairs.** The environment is where an MCP server's credentials live, and those are exactly what
`AGENTS.md` forbids in a configuration file. Requiring a reference per entry means a credential **cannot be
written into a profile at all** rather than being discouraged by a comment — and because the reference grammar
already refuses a locator outside the `JARVIS_` prefix, a profile also cannot name an unrelated ambient secret
such as a cloud credential. `McpLaunchSpec` still takes literal pairs; the *composition* resolves the
references, so no literal ever arrives from a file. The rule is asserted through **TOML text**, because a
profile is what an operator writes and a struct literal would bypass the deserializer where the refusal lives.

Two further decisions worth recording:

- **Both name rules are checked before anything is spawned.** `ServerConfigId` accepts a leading digit; a tool
  source owner requires an initial letter. A name satisfying one and failing the other produces an all-refused
  catalog at discovery — a server-shaped symptom of a name-shaped cause.
- **A duplicate name is refused, not deduplicated.** The name is the trusted identity every registration and
  grant is recorded against, so two servers claiming it would register one's tools under an identity an
  operator believed belonged to the other, and the second would then fail with a source collision the operator
  never caused.

The program is required **resolved**, with deliberately no command-plus-arguments shorthand: `env_clear`
removes `PATH`, so a bare `npx` cannot be resolved by the child and must be resolved by the daemon at
configuration time. A shorthand would read as convenience and fail at spawn with `SpawnFailed`, naming the
child rather than the configuration.

Three mutations falsify the section, each in its own test: dropping the owner rule, dropping the duplicate-name
check, and filtering disabled declarations **before** validating them.

### Composition: the chain, and the fan-out the router cannot do

`mcp/composition.rs` runs the whole chain in order — resolve, spawn, handshake, list, normalize, register,
publish what was admitted, dispatch by identity — and is exercised end to end against spawned children in
`tests/mcp_composition_process.rs`.

**The fan-out has to live inside the source kind, and that is a consequence rather than a preference.** Every
MCP server's tools declare `SourceKind::McpServer`, and `RoutingExecutor` holds **at most one** executor per
kind by construction (two for one kind would leave the choice to insertion order). So a profile with two
servers cannot dispatch through the router, and `ComposedMcpExecutor` fans out *inside* the kind, keyed by the
**whole identity**: the identity includes the source owner, and registration already compares that owner
against the trusted configuration identity, so an identity belonging to one server cannot match another's
admitted set. Dispatch is a membership test rather than a string comparison, and there is no ambiguity for a
lookup order to resolve. An identity no server admitted is `NotFound`, matching the router's own answer for an
unrouted kind.

**One failing server does not abort the others.** A profile may declare several and the failure modes are
per-server, so aborting on the first would let one typo take down every server an operator configured.
`McpComposition` returns the composed servers *and* the refusals with their reasons, because "three declared,
two running, one token unresolvable" is the operator fact — and a *profile-level* fault (a duplicate name, an
unusable identity) is still refused before anything spawns, since the declaration's own validation catches it.

**The order is `enabled`, then resolve, then spawn.** Secret material is resolved at "the last responsible
moment", which for a launch is immediately before the spawn — resolving later is impossible because the child
needs it in its environment. Checking `enabled` first means a server an operator switched off never has its
secret read, and an unresolvable reference is reported as the *secret* code rather than as a server that failed
to start.

⚠ **Two of my own tests were wrong first, and the mutations found both** — the third time this has happened in
this session. `a_disabled_server_is_never_spawned` passed the **already-filtered** list, so it proved that
`enabled_declarations()` filters rather than that the composition skips; the ordering claim was untested until
it passed `declarations()` instead, at which point the mutant failed. And the recording double was a `match`
with one catch-all arm — clippy flagged it as replaceable by its scrutinee, which is a lint *and* a real
defect: an arm matching everything means the binding was never used, so the comment claiming it "records which
identity it was asked for" was false. **A mutation caught by the wrong test means the fixture, not the guard.**

### The daemon composes them, and the old single-function fabric was split rather than kept

`daemon::start` now resolves the profile's `[mcp]` declarations, spawns and registers each server, and builds
the executor the pipeline dispatches through from **one** router carrying whatever kinds the daemon can serve.
`jarvisd`'s composition root validates the declarations before the daemon is built, so a broken name or
environment key fails startup rather than surfacing as a server that "failed to start" — and a **disabled**
declaration is validated too, because it is a reviewed record that must stay correct.

**The structural decision was deleting `tool_fabric_over` rather than keeping it.** The pipeline's executor
slot is single, so MCP dispatch requires the router to be supplied *from outside* — which splits the old
function in two: `router_over` chooses the kinds, `tool_fabric_with` builds everything that does not depend on
where a tool comes from. Clippy then reported the old function as never used, because the daemon had stopped
calling it. Silencing that would have left two compositions, so instead the journey tests were moved onto the
same two functions: **one wiring the daemon and the journeys share**, and no second composition a journey
could pass while the daemon's own was broken.

Two smaller decisions worth recording:

- **The kind gate is asserted through the production path.** `router_over(&clock, &[])` is what a profile with
  no `[mcp]` table gets, and the test asserts it routes `Native` **only** — a router advertising `McpServer`
  with no server would claim a source the daemon cannot serve. Falsified by registering the MCP kind in the
  empty branch. A second test asserts the daemon's router actually *resolves* a native tool the catalog
  offers, so a kind mismatch that would refuse every dispatch is caught here rather than appearing as
  `tool.not_found` for a tool the catalog lists.
- **`router_over` returns the concrete `RoutingExecutor`, not a trait object.** That is what makes
  `routed_kinds()` assertable; `Arc<dyn ToolExecutor>` would hide the entire decision — which kinds are
  registered — behind a trait with no method to ask. A concrete type for a value a test must interrogate, and
  a trait object only where the caller genuinely does not care.

A per-server failure is **logged, not fatal**: one unreachable server must not deny the daemon its own tools,
so each refusal is reported at `warn` with its stable code — never the message, which may carry operator text.

### Drain: stopping the children the daemon started

The composition's owner was the wrong one, and finding that required asking who held it. `start` built the
`McpComposition` as a **local** and handed only the router to the pipeline, so the composition was dropped when
`start` returned while each child stayed alive through the *dispatcher's* `Arc<McpToolExecutor>`. Nothing could
ask a server to stop, which means a daemon exiting normally would leave every MCP child running — orphaned
rather than shut down. `RunningDaemon` now holds `Arc<Mutex<Option<McpComposition>>>`, and `serve_until` stops
them on the serve-error path, the drain-failure path, and the normal drain path.

**The `Option` is the mechanism, not decoration.** `serve_until` can reach the stop step twice for one
composition — once on a serve error and again on the way out — so the holder is *taken*, which makes the second
call a no-op instead of a second cancel of a session that has already stopped. Both directions are asserted:
`stopping_the_servers_twice_stops_them_once` requires the holder to be empty after the first call and silent
after the second, and `a_drain_with_no_servers_stops_nothing` covers the `None` holder that nearly every real
run has.

**The stop is `cancellation_token().cancel()`, and that form is forced.** `RunningService::cancel(self)`
consumes the service and `close` needs `&mut`, and neither is reachable: the session `Arc` is shared with the
dispatcher's executor, so it can be neither moved out of nor borrowed mutably. Three compile errors (`E0507`
among them) established that before the token was tried. The token owns a clone of the signal, which is the one
non-consuming stop. The cost is that it does not await, so `shutdown` **polls `is_closed` to a one-second
bound** and reports any server it could not confirm stopped — a cancel that silently assumed success would be
the class of unverified claim the drain exists to remove.

**`McpComposition::health` is the lifecycle probe, and it reads the session rather than the assumption.** A
composed server is a child process, so "is it still there" has to come from `is_closed()` and the protocol
version from `peer_info()`; a child that died after composition leaves a handle that looks healthy until
something asks it. Deliberately **not** a boolean `healthy`: a closed session is a fact to report beside the
tool count and the negotiated version, not a failure the daemon can repair.

**Falsified by making the cancel a no-op**, which fails exactly the two new e2e tests
(`shutting_a_composition_down_closes_every_session`, `a_lifecycle_probe_records_each_servers_health`) and no
others — the detector sits where the claim is. The e2e shutdown test keeps the dispatcher **alive across the
stop**, because that is the arrangement a daemon has; a shutdown needing sole ownership would fail there, which
is what makes it a test of the real shape rather than of a convenient one.

**Still open.** A child that dies *during* a run is not yet noticed and quarantined — `health` reports it but
nothing acts on it; there is no heartbeat or idle supervision beyond the drain. Deregistration when a server is
removed from configuration is unnecessary *today* because the registry is built per start and dropped, and
becomes necessary the moment the registry is held.

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
| Header mismatch | `HeaderMismatchError` (-32020) | No | Adapter bug; fail the call and surface a diagnostic, do not retry. Classified `tool.unavailable` — refused at the door, so settled |
| Missing client capability | `MissingRequiredClientCapability` (-32021) | No | Do not silently downgrade the request; report the unmet capability. Classified `tool.unavailable` for the same reason |
| Authorization required | HTTP 401 / challenge / typed SDK error | After auth flow | Do not loop; surface setup/reauth. Validate `iss` before redeeming |
| Tool schema invalid | Invalid list/call data | No | Hide tool, degrade server |
| Resource not found (legacy) | `-32002`, from a `2025-11-25` or earlier peer | No | Classify `tool.not_found`. Clients **SHOULD** still accept it, and discovery accepts an older peer by design, so this path is reached |
| Resource not found (current) | `-32602` after the SEP-2164 renumbering | No | **This number now carries two meanings** and only the message distinguishes them: classify `tool.schema_invalid`, because the argument meaning is the one a caller can act on, and no *reserved-resource* class exists in the fabric yet |
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
- [x] Error mapping for `-32020`, `-32021`, `-32022`, and `-32602` — plus the legacy `-32002`. The
  distinction that matters is **whether the peer processed the request**: every door-refusal is settled
  (`-32022`/`-32020`/`-32021` → `tool.unavailable`; `-32602`/`-32600`/`-32700` → `tool.schema_invalid`;
  `-32601`/`-32002` → `tool.not_found`), while `-32603`, or any code the table does not recognise, is
  `tool.provider_error` and **unsettled** because the peer may already have executed the request.
  Falsified by restoring either capability/header code to the catch-all, which fails
  `a_request_refused_at_the_door_is_never_reported_as_unsettled` naming the code
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
| 2026-10-03 | `discovery.rs`: the spawn-to-catalog caller, verified against a real child process. Corrected three assumptions — discovery accepts a peer one revision behind (no floor), an all-refused catalog is a named refusal rather than an empty one, and the server identity cannot be derived from a launch spec. Recorded the per-connection response cache that `list_tools` consults | Pinned SDK sources: `src/service/client.rs` (`serve_with_discover`, `list_tools`, `list_response_cache_key`), `src/service.rs` (`ServiceError`), `src/model.rs` (`DiscoverResult`, `ServerPeerInfo`), `src/handler/client.rs` (`ClientHandler` default), `src/transport/io.rs` (`stdio`) |
| 2026-10-03 | Corrected two claims in the error mapping. (1) `-32020`/`-32021` were classified `tool.provider_error` — **unsettled and retryable for an idempotent tool** — for requests the peer refused before dispatching; both are now `tool.unavailable` with the version refusal, so a call that provably never ran cannot enter reconciliation. (2) The module doc's "every code the specification defines is mapped" was true of the nine named codes and read as "every code"; the reserved range is only partly enumerable, so the wording now states the nine and why the rest cannot be. Also recorded that resource-not-found moved from `-32002` to `-32602` (SEP-2164) and that `-32602` therefore carries two meanings | Official spec `https://modelcontextprotocol.io/specification/2026-07-28/basic/index.md` (Error Codes: the `-32000`..`-32019` legacy vs `-32020`..`-32099` reserved partition; "Clients SHOULD still accept `-32002`"; required `_meta` fields rejected `-32602` with HTTP `400`); `.../changelog.md` minor change 6 and 12; pinned SDK `rmcp 3.5.0` `src/model.rs` (`ErrorCode` constants at lines 624-633, `ErrorData::resource_not_found` doc "upgraded to `INVALID_PARAMS` (`-32602`)"), `src/service/client.rs` (`ClientRequestMetadata` seeding at 281/929) |