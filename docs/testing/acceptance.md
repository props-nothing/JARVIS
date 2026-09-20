# Acceptance Scenarios

Status: PROPOSED

These stable IDs define product proof. Individual tests may split a scenario by
backend, provider, or operating system.

## Foundation

### `ACC-001`: Clean Local Install

On clean Windows x86_64, macOS arm64/x86_64, Linux x86_64, and Linux aarch64,
install packaged JARVIS without a developer toolchain. Start the per-user daemon,
run `jarvis status` and `jarvis doctor`, then uninstall while retaining data.

### `ACC-002`: Local API Isolation

An enrolled CLI can call the daemon. A process without the credential, a
malicious browser origin, an invalid Host, and a revoked device cannot.
Unauthenticated loopback liveness/readiness probes are the sole exception and
return only their contract status token; they cannot enumerate identity,
workspace, version, configuration, errors, or capabilities.

### `ACC-003`: Restart and Repair

Terminate the daemon uncleanly, start it again, recover locks/workers, and retain
valid state. Seed port, service-path, config, permission, and storage faults;
doctor identifies them and a confirmed repair verifies postconditions.

### `ACC-004`: Update and Rollback

Install a signed lower-version compatibility fixture or prior supported package,
create state through public APIs, update to the candidate, verify migration and
readiness, and exercise failed activation. Old binary refuses newer unsupported
state without modifying it.

### `ACC-005`: Backup and Restore

Back up populated local state, verify manifest/hashes and secret exclusions,
restore into a new profile, and prove conversations/memory/workflows while
connectors that lack secrets require reauthentication.

### `ACC-006`: Profile and Instance Isolation

Create two local profiles and concurrent/stale daemon discovery states. Each CLI
connects only to its selected authenticated instance; owner-only files reject
unsafe permissions, and hostile discovery URLs, PID reuse, wrong-profile
credentials, origins, and hosts fail closed with actionable diagnostics.

### `ACC-007`: Configuration Compatibility

Exercise defaults, file, environment, and command-line precedence without
placing secrets in ordinary config. Interrupt an atomic write, load malformed or
newer unsupported versions, migrate an older fixture, and prove the old binary
does not rewrite unsupported state.

### `ACC-008`: Daemon Execution Modes

Run the same health/status/scripted-request proof in foreground, per-user service,
portable, and server-container modes where supported. Each mode uses explicit
profile/storage paths, one durable daemon authority, and the same public contract;
stopping one mode cannot attach to or mutate another profile accidentally.

### `ACC-009`: Product Lifecycle and Data Choice

From packaged artifacts, install, repair, back up, restore, update, exercise
failed activation/rollback, and uninstall with both retain-data and explicit
purge choices. Interrupted and repeated commands are idempotent, connector
secrets are not copied into backups, and every retained path is reported.

## Brain

### `ACC-010`: Deterministic Streamed Chat

Start a CLI chat through a real daemon/SQLite and scripted provider. Observe
ordered activity/text deltas, exactly one terminal event, persisted session/run,
and reproducible output.

### `ACC-011`: Provider Smoke

With explicit test credentials, stream one response from the selected first
provider, record usage/request ID, cancel a second call, and expose safe failure
when credentials are invalid. No secret appears in logs/support output.

### `ACC-012`: Client Disconnect

Disconnect the streaming client mid-run. The run follows declared cancellation
or continuation policy, remains queryable, and never becomes false `Completed`.

### `ACC-013`: Model Fallback Policy

A primary transient failure uses an allowed capability/privacy-equivalent
fallback. Explicit pin, local-only data, refusal, or incompatible capability
prevents fallback with an explainable decision.

### `ACC-014`: Public Reasoning Boundary

Feed the scripted provider hidden reasoning, provider internals, prompt content,
and safe summary events. Public streams, persistence, logs, support output, and
errors contain only allowed activity/evidence summaries and never hidden chain-
of-thought, secrets, or system-policy text.

### `ACC-015`: Provider Separation and Replacement

Run the same normalized request through two fixture provider adapters. Domain
and persisted records contain JARVIS types, capability routing remains explicit,
and replacing one adapter requires no conversation/run schema or client change.

### `ACC-016`: Conversation Runs and Bounded Cancellation

Create two durable runs in one conversation and prove independent ordered event
and result histories. Cancel runs during context, provider startup, streaming,
and cleanup; cancellation intent persists before signalling, cleanup is bounded,
the terminal state is truthful, and repeated cancellation is idempotent.

### `ACC-017`: Complete Model Routing, Retry, and Ownership

Use a deterministic candidate matrix to vary capability, reasoning need, latency,
cost, privacy/locality, context length, modality, tool/structured-output quality,
availability, and explicit user policy. The explainable selected route obeys all
hard constraints. Retry keeps one logical call owner/idempotency identity, bounds
attempts/backoff, never retries terminal input/auth/refusal errors, and records
each provider attempt without duplicating a run result or side effect.

### `ACC-018`: Visible Model Data Policy

Configure local-only, no-provider-retention where documented, telemetry-off,
allowed-provider, sensitivity, residency, and explicit exception policies. The
UI/API shows requested and effective behavior plus current provider evidence;
routing rejects candidates that cannot satisfy hard policy, records the reason,
and never claims a provider guarantee that is `UNVERIFIED`.

## Tools and MCP

### `ACC-020`: Approval Survives Restart

Request an approval-required fake communication tool, stop the daemon while
waiting, restart, approve from the correct client, and complete exactly one
effect. Changed arguments, expired approval, wrong principal, and duplicate
submission fail.

### `ACC-021`: Route Equivalence

Invoke equivalent native, MCP-imported, runtime, and API tool intents. Every call
produces the same validation/policy/approval/audit stages and cannot bypass deny.

### `ACC-022`: MCP Client

Connect to official/reference stdio and Streamable HTTP servers, negotiate the
supported version, list tools, call one, handle auth/cancel/progress/error, and
survive server restart or quarantine.

### `ACC-023`: MCP Server Export

An external MCP client authenticates, discovers only its allowed read tool, calls
it, and cannot discover/call an ungranted write tool. Revocation takes effect on
the next operation.

### `ACC-024`: Tool Identity Replacement

Replace an MCP/plugin tool schema/source behind the same display name. Existing
approval/grant cannot authorize the replacement.

### `ACC-025`: Ambiguous Side Effect

Crash after the fake provider accepts an effect but before result persistence.
Recovery enters reconciliation and proves no duplicate effect.

### `ACC-026`: Extension Manifest and Quarantine

Install a signed fixture extension manifest without granting capabilities,
reject incompatible/tampered/unknown-permission manifests, explicitly grant one
scope, then exercise crash, hang, output flood, upgrade, disable, quarantine, and
remove while the daemon remains healthy.

### `ACC-027`: Approval Channel Assurance

Issue the same exact-action approval to CLI/API, desktop, mobile, and voice
channels as each becomes supported. Only a currently authenticated, policy-
allowed channel with sufficient principal/device/session assurance can decide;
cross-channel replay, stale notification, changed preview, and downgraded voice
identity fail. Revocation takes effect before execution.

## Memory and Context

### `ACC-030`: Remember Across Restart

User says, "Remember that client proposals should be concise." Confirm/save,
restart, ask for the preference, and receive it with provenance.

### `ACC-031`: Correct and Forget

Correct a remembered preference, inspect lineage, and export it with provenance.
Disable memory and prove no extraction, retrieval, context inclusion, or new
candidate persists; re-enable explicitly. Then hard-delete the preference. It no
longer appears in lexical/semantic/entity retrieval, context, cache, export,
derived relation, backup subject to deletion policy, or provider-side artifact
that JARVIS claims to control.

### `ACC-032`: Workspace Isolation

Create identical and semantically similar memories/documents in two workspaces.
Queries, vector search, context manifests, caches, and exports never cross scope.

### `ACC-033`: Untrusted Memory Candidate

A retrieved document instructs JARVIS to remember a false sensitive fact and
send data. No durable memory or unauthorized tool action occurs.

### `ACC-034`: Typed Memory Provenance

Exercise working, conversational, episodic, semantic, preference, relationship,
and procedural memory with source, scope, confidence, sensitivity, validity, and
lineage appropriate to each type. Working state expires without becoming
durable by default; inferred sensitive facts and relationship merges require
their configured confirmation threshold. Invalid combinations fail, and
uncertainty is not silently converted into permanent truth.

### `ACC-035`: Context Selection Ledger

Build context under a fixed token budget with conflicting, expired, sensitive,
and cross-workspace candidates. Every included item records source and reason;
policy filtering precedes ranking, budget is enforced, and excluded sensitive
content never reaches the provider request.

### `ACC-036`: Hybrid Memory Ranking

Use a versioned evaluation corpus to vary semantic similarity, lexical match,
entity relevance, recency, importance, source reliability, task relevance, and
contradiction independently. The versioned score breakdown and candidate sets
show each enabled signal changes ranking as specified, entity false-merges remain
separate, workspace/policy filtering occurs before candidate ranking, and no
single embedding score silently replaces the hybrid policy.

## Connectors and Automation

### `ACC-040`: Connector Lifecycle

Configure a dedicated test account, validate scopes, activate, read one resource,
expire/revoke auth, reauthenticate, disable/unload, migrate, remove remote
subscription, and produce redacted diagnostics.

### `ACC-041`: Webhook Integrity

Accept one valid delivery, reject invalid/stale signatures, deduplicate replay,
handle out-of-order events, acknowledge within deadline after durable inbox, and
process asynchronously.

### `ACC-042`: Durable Scheduled Approval Workflow

Schedule a multi-step workflow, terminate at timer, model/activity, approval, and
external-effect boundaries, then restart. It completes one effect with correct
history and no duplicate event.

### `ACC-043`: Proactive Policy

A relevant event during quiet hours prepares a briefing but does not interrupt
or act. Outside quiet hours it notifies within cooldown/budget. Revoking the rule
prevents pending future action.

### `ACC-044`: Outbox, Replay, and Consumer Deduplication

Fail transactions before/after state and outbox commit, redeliver and reorder
events, crash the consumer before/after its handling record, and replay an
authorized range. State and claimed events never diverge; duplicate handling
does not duplicate effects.

### `ACC-045`: Connector Completeness and Backpressure

Against a captured/fake provider contract, traverse pagination and incremental
sync, inject gaps, duplicate/out-of-order pages, expiry, rate limits, partial
failure, and cursor invalidation. The connector reports freshness/completeness,
resumes safely, and never silently claims a partial view is complete.

### `ACC-046`: Workflow Step and Wait-State Matrix

Execute sequence, bounded parallel fan-out, condition/switch, timer, correlated
event wait, approval wait, agent-run wait, retry, timeout, cancellation, and
compensation steps. Terminate the process before and after every wait/transition,
race two leased workers, reorder/duplicate wake events, and cancel in each state.
Recovery follows the pinned definition version, compensation is a recorded best-
effort action, and no branch or side effect executes more than policy permits.

## External Runtimes

### `ACC-050`: Runtime Interchangeability

Run the same safe objective through native and fixture external runtimes from one
client. JARVIS state/events/tools remain canonical while runtime-specific events
are normalized.

### `ACC-051`: Runtime Failure Containment

Exercise incompatible version, malformed event, sequence gap, output flood,
hang, crash, checkpoint mismatch, and repeated failure. The daemon stays healthy,
run state is truthful, and quarantine/diagnostics are actionable.

### `ACC-052`: Runtime Scoped Tools

Runtime's ephemeral MCP/capability credential can call one selected tool only for
its run/workspace and fails after run completion/revocation.

## Voice

### `ACC-060`: ElevenLabs Custom LLM Text/Voice Turn

Using a dedicated ElevenLabs test agent, call the authenticated Responses edge,
stream a spoken answer through JARVIS, and verify session/run/context/tool/audit
mapping. Run the Chat Completions compatibility case separately if supported.

### `ACC-061`: ElevenLabs MCP Scope

An ElevenLabs-owned agent connects to JARVIS MCP, sees one permitted read tool,
cannot access a private/high-risk tool, and remains subject to JARVIS approval.

### `ACC-062`: Inbound Caller Assurance

Known, unknown, expired-token, and spoofed-caller scenarios receive appropriate
assurance. Only step-up verified sessions can access private calendar/memory.
Every accepted call remains associated with the server-resolved configured route
workspace; route metadata and caller-provided workspace fields cannot change it.

### `ACC-063`: Outbound No-Double-Ring

An approved urgent workflow starts one test call. Duplicate command, timeout,
daemon crash, and duplicate/out-of-order callbacks never create a second call for
the same idempotency key.

### `ACC-064`: Voice Privacy

Recording/transcript disabled, enabled-with-retention, redaction, export, delete,
and provider callback paths match policy. Audio never appears in logs/support.

### `ACC-065`: Voice Provider Replacement and Degraded Mode

Run a normalized text/voice session through fixture voice providers, disable the
active provider during a call, and restart without voice configured. JARVIS core,
canonical session state, CLI, tools, and policy remain usable; provider-specific
state stays behind the adapter.

### `ACC-066`: Outbound Call Policy

Attempt outbound calls with absent/expired consent, prohibited region/purpose,
quiet hours, exhausted budget, missing approval, disabled provider, and allowed
urgent policy. Only the fully allowed case dials, with recorded reason and exact
idempotency key; revocation before provider acceptance prevents the call.

### `ACC-067`: Voice Conversation Controls

Exercise user/provider interruption, barge-in, explicit end, remote hangup,
transfer success/failure, voicemail detection, partial transcript, callback
reordering, timeout, and reconnect where supported. Each becomes an explicit
normalized transition with bounded cleanup and truthful final outcome; partial
text never becomes a fabricated completed instruction.

## Security and Operations

### `ACC-070`: Secret Canary

Seed unique key/token/password values through config, provider errors, runtime
stderr, webhooks, model/tool payloads, and voice metadata. Scan logs, traces,
metrics, API errors, doctor output, support bundle, and installer logs; find none.

### `ACC-071`: SSRF and Filesystem Escape

Adversarial URLs/redirects/DNS and paths/symlinks/junctions/reserved names cannot
reach forbidden network/file targets on each supported OS.

### `ACC-072`: Signed Supply Chain

Valid packaged artifact installs. Modified artifact/manifest/signature and
downgrade outside policy fail before activation. Failed update retains the old
working version and user data.

### `ACC-073`: Budget Exhaustion

Adversarial model/tool/runtime/workflow/voice loops hit configured turn, token,
cost, time, byte, retry, and concurrency limits with typed outcomes and no
unbounded resource growth.

### `ACC-074`: Contract and Persisted Compatibility

Read golden public/persisted fixtures from every supported prior version, ignore
allowed additive fields, reject unknown security-critical values, and refuse an
unsupported major without mutation. Generated schemas and clients have no drift
from their owning source.

### `ACC-075`: Offline Local Privacy

Run onboarding, scripted chat, memory, backup, restore, doctor, and support-bundle
preview with network denied and telemetry disabled. No unexpected DNS/network
attempt occurs, and every unavailable cloud feature is explicit rather than
silently retried or enabled.

### `ACC-076`: Correlated Safe Diagnostics

Trace one request through run, model, tool, approval, workflow, and failure
boundaries. Allowed correlation IDs join records while private payloads and
secrets remain absent; doctor/support output gives a user-safe path to the same
failure without exposing internal credentials or cross-workspace data.

### `ACC-077`: Server Users and Service Clients

Create two users, devices, sessions, workspaces, and a least-privilege service
client. Exercise login/enrollment, workspace switch, expiry, rotation, global and
single-client revocation, step-up, disabled membership, and forged subject/
workspace fields. Every operation resolves identity and scope server-side; no
credential or cache entry crosses users/workspaces.

### `ACC-078`: Reviewable Support Bundle

Seed diagnostics across daemon, storage, model, runtime, tool, connector,
workflow, and voice boundaries. The user previews an itemized bundle, can exclude
optional artifacts, and exports only allowlisted redacted data with manifest and
retention notice. Credentials, private keys, raw prompts/payloads, recordings,
cross-workspace data, and unbounded child logs never enter the bundle.

### `ACC-079`: Durable Aggregate and Migration Matrix

For every durable aggregate, inject failure before and after state commit,
outbox/event publication, acknowledgement, and side-effect reservation. Upgrade
from every supported prior schema/payload/API fixture on SQLite and PostgreSQL
where supported, then exercise backup/restore and failed activation rollback.
No aggregate becomes falsely terminal, loses workspace ownership, duplicates an
effect, or is silently rewritten by an incompatible older binary.