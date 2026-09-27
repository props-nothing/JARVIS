# Conceptual Relational Schema

Status: PROPOSED
Last updated: 2026-09-20

## Conventions

- Primary IDs are typed UUIDv7 strings once the implementation dependency is
  verified.
- All tables containing workspace-owned data or authority carry `workspace_id`
  directly, even when it could be inferred through joins. This supports safe
  query APIs and composite constraints.
- Global principal roots (`users`, external `user_identities`, and devices) may
  span workspaces but contain no workspace-private payload or grant. Membership,
  session active scope, credential, policy, and resource rows bind their use to
  a workspace.
- Truly global definitions/operational metadata declare system scope and cannot
  contain tenant payloads, references, embeddings, credentials, or authority.
- Mutable aggregates carry `version` for optimistic transitions.
- Timestamps are UTC and include `created_at`; mutable rows include `updated_at`.
- Soft deletion is used only when restore/audit semantics require it. It is not a
  substitute for actual deletion workflows.
- Extensible payloads include a schema version and are size-bounded.
- Secret values never live in these tables; rows store `secret_ref` identifiers.
- Large content lives in artifacts/object storage with relational metadata.

## Identity and Workspaces

### `users`

```text
id, status, display_name, locale, timezone, created_at, updated_at
```

### `user_identities`

```text
id, user_id, issuer, subject, identity_type, metadata_json, created_at, last_used_at
UNIQUE(issuer, subject)
```

External subjects are issuer-scoped.

### `workspaces`

```text
id, kind, name, status, policy_version, retention_policy_id,
created_at, updated_at
```

### `workspace_memberships`

```text
workspace_id, user_id, role_id, status, created_at, updated_at
PRIMARY KEY(workspace_id, user_id)
```

### `roles`, `capability_grants`, `policy_bindings`

Roles bundle grants. Grants bind subject, workspace, capability/resource,
constraints, validity, issuer, and revocation. Policy bindings reference a
versioned deterministic policy.

### `devices`

```text
id, user_id, name, platform, credential_public_ref, status,
enrolled_at, last_seen_at, revoked_at
```

### `client_credentials`

```text
id, workspace_id, principal_type, principal_id, device_id nullable,
service_client_id nullable, credential_family_id, verifier_algorithm,
verifier_hash, scopes_json, status, issued_at, expires_at,
last_used_at, revoked_at, rotated_from_id nullable
```

The plaintext opaque credential is held by the client in an OS credential store
or approved protected fallback. JARVIS stores only a one-way verifier. Constraints
bind the credential to exactly one valid principal/client subject and workspace;
rotation creates a new row and revokes the old family member without changing
historical attribution.

### `sessions`

```text
id, principal_type, principal_id, device_id, active_workspace_id,
channel, assurance, status, issued_at, expires_at, revoked_at,
credential_family_id, metadata_json
```

Session credential hashes/references are protected and separated as required by
the auth implementation.

### `service_clients`

```text
id, workspace_id, name, client_type, status, credential_ref,
allowed_protocols_json, issued_at, expires_at, revoked_at
```

`credential_ref` identifies asymmetric key material or a secret-manager record
when that client type requires it. Bearer credential verifiers use
`client_credentials`; no plaintext credential is stored in either table.

### `api_idempotency_records`

```text
id, workspace_id, principal_id, client_credential_id, api_major,
operation, key_digest, request_fingerprint, state,
resource_type nullable, resource_id nullable,
http_status nullable, response_ref nullable, error_code nullable,
created_at, updated_at, expires_at
UNIQUE(workspace_id, principal_id, client_credential_id,
  api_major, operation, key_digest)
```

The key digest and canonical request fingerprint are recorded before or in the
same transaction as the acknowledged mutation. Reuse with different material
input is a conflict. Completed and ambiguous outcomes remain long enough to
cover client retries and downstream reconciliation.

**The implemented table is narrower than the sketch above, and the sketch is aspirational.** The local
control API creates `idempotency_records` (`000003`, rescoped by `000007`) with:

```text
id, idempotency_key, workspace_id, principal_id, client_credential, operation,
api_major, request_digest, run_id, created_at
UNIQUE INDEX on (principal_id, workspace_id, client_credential, operation,
  api_major, idempotency_key)
```

Two differences from the sketch are deliberate rather than pending. There is one resource type, a run,
so `resource_type`/`resource_id` collapse to a foreign-keyed `run_id`. And the outcome is always
`202 Accepted` with a completed mutation, because the record commits in the **same transaction** as the
run — so the `state`, `http_status`, `response_ref`, and `error_code` columns the sketch reserves for
in-flight and ambiguous outcomes have nothing to hold. They become necessary when a command with an
externally visible side effect is acknowledged before it completes; nothing on this surface is.

`000007` narrowed the uniqueness key from three dimensions to the contract's five. The first version
keyed on workspace, operation, and API major while its own comment claimed the full scope, and because
a local profile shares **one** workspace between every enrolled client that let two clients' keys
collide — with the replay path resolving the run by workspace alone, so the second client received the
first client's `run_id` and `conversation_id`. `client_credential` holds a **digest**; the credential
itself is never stored. A row written before `000007` is attributed from its run's `principal_id` and
carries the `unknown` credential sentinel, so it never matches on that dimension — the fail-closed
direction, since nothing recorded which credential created it.

## Conversations and Agent Runs

### `conversations`

```text
id, workspace_id, owner_user_id, title, status, channel_origin,
created_at, updated_at, archived_at
```

Named `conversations`, not `sessions`. The
[identity architecture](../architecture/identity-workspaces.md) reserves
**session** for a bounded authenticated interaction context (the `sessions` table
above and its channel/assurance columns), while a conversation is the durable
transcript container. The local control API addresses this aggregate as
`conversation_id`. `jarvis-domain` previously carried a `SessionId` documented as
identifying a "durable conversation session", which conflated the two; it is now
`ConversationId`, and `MessageId` was added alongside it.

### `messages`

```text
id, workspace_id, conversation_id, role, content_schema_version,
content_ref_or_json, sensitivity, source, sequence, created_at, deleted_at
UNIQUE(conversation_id, sequence)
```

Tool calls/results use typed content items and stable pairing IDs.

### `agent_runs`

```text
id, workspace_id, conversation_id, parent_run_id, principal_id,
objective_ref, state, version, runtime_id, runtime_version,
context_manifest_id, plan_summary_ref, waiting_kind, waiting_ref,
deadline_at, budget_json, result_ref, error_code, error_ref,
created_at, started_at, updated_at, completed_at
```

State and waiting fields have constraints preventing incompatible combinations.

`runtime_id` and `runtime_version` name the runtime that *executed* the run and the build version it
was, so a resume can validate both against the runtime it is about to use
(`docs/architecture/agent-runtime.md`). They are written by the create path and read back by the
same port: before that, the `CreateRunRequest.runtime` field was required and validated and then
discarded, so both columns were referenced by no code at all and every row carried `NULL` while the
contract expected a value. Both are nullable because a row written before they had a writer has no
truthful value to give, and a row carrying **one** of the pair is reported as corruption rather than
as absent — half an identity cannot be validated against anything.

### `agent_steps`

```text
id, workspace_id, run_id, sequence, kind, state, version,
idempotency_key, input_fingerprint, input_ref, output_ref,
attempt_count, max_attempts, timeout_ms, next_attempt_at,
error_code, created_at, started_at, completed_at
UNIQUE(run_id, sequence)
UNIQUE(workspace_id, run_id, idempotency_key)
```

### `run_activity_events`

Append-oriented public activity projection with run sequence, event type,
payload reference, visibility, and timestamp. It is not the private model trace.

### `model_calls`

```text
id, workspace_id, run_id, step_id, logical_call_id, attempt,
provider_id, model_id, model_revision, route_decision_id,
state, request_fingerprint, provider_request_id, continuation_ref,
usage_json, estimated_cost_microunits, finish_reason,
error_code, started_at, first_output_at, last_output_at, output_delta_count, completed_at
UNIQUE(logical_call_id, attempt)
```

`route_decision_id` names the authorization that permitted the call, `finish_reason` is the
provider's own account of why it stopped, and `provider_request_id` is the provider's own
identifier, which is what correlates a JARVIS call with a provider-side record. All three are
**read back** by the same port that writes them — a column no `SELECT` names is a value that can be
written and never observed, which is how each of these was once stored as `NULL` on every row. The
provider id is attached to a stream frame's metadata rather than to a specific event kind, so the
write was missing in a different place from the other two: the controller's frame fold never read
`provider_metadata` at all.

`first_output_at` is the instant the **first** output delta arrived, observed from JARVIS's own
clock so it is comparable across providers, and it is what a time-to-first-token interval is
computed from. It was the last column in this group to gain a producer: the port, the bind, the
`SELECT`, and the double all carried it while every controller path wrote `None`, so the column was
`NULL` on every row and nothing could measure the interval the roadmap asks for. A call that emits
output and then fails still records it, because the tokens were really produced; a call refused
before the provider accepted it does not, which is why those two paths pass `None` explicitly rather
than by omission.

`last_output_at` and `output_delta_count` complete that measurement (`BRN-011`). One instant and a
first token cannot distinguish a genuine stream from a burst — a model that advertises streaming and
delivers its whole reply at once looks identical at the start — so the interval needs its **end** and
the count needs its **sample size**. `last_output_at` is the last delta's instant and
`output_delta_count` the number of deltas, and `IncrementalDelivery` is **derived** from the three
values at read time rather than stored: a stored verdict would be a second answer to "was this a
burst", and a change to the domain's minimum-spread constant would silently not apply to rows already
written. Both are `NULL` for a row written before the migration **and** for a call that delivered no
output, and neither is `0`: `NULL` means "no measurement was taken" while `0` would mean "measured,
and the answer is zero deltas", and the two must stay apart — the same "unknown is not zero" rule
the usage counters follow. The count is written only when a first-output instant exists, so a count
without an instant is a shape the writer cannot produce.

`ModelCallRepository::delivery_campaigns` reads those three columns into one profile per call and
groups them per `(provider, model, revision)` **for a measurement campaign**, which is how a model's
delivery profile is aggregated (`BRN-011`). The read filters out any call whose timing columns are
`NULL`, so a call that produced nothing cannot be folded in as a zero and invent a burst for a model
that was never allowed to stream — the same `NULL`-is-not-zero rule applied at the group boundary.
The three `IS NOT NULL` clauses are written separately although the write path always sets all three
together; that redundancy guards against a row this binary did not write, and the consequence is
recorded rather than implied: **removing one clause changes no result**, so the falsification that
means something is removing all three. Groups are returned even when they are too small to attest
anything, because the sufficiency floor is a rule about what the samples *mean* and belongs where it
can be tested (`DeliverySamples::aggregate`), not hidden in a `HAVING COUNT(*) >= 3` a test could only
observe through a caller.

Prompt/output content uses protected artifact/content references under retention
policy rather than being duplicated in telemetry rows.

### `model_route_decisions`

Records requirements, candidates, selected route, rejected reason codes,
fallback chain, model data policy ID/version, requested/effective data policy,
provider capability evidence references, exception reference, policy version,
and safe metadata.

### `model_data_policies`

```text
id, workspace_id, owner_principal_id nullable, name, version, status,
rules_schema_version, rules_json, created_by, created_at, activated_at,
superseded_at
UNIQUE(workspace_id, id, version)
```

Policy versions are immutable. One active version per policy identity/workspace
is selected through an optimistic transition.

### `model_policy_exceptions`

```text
id, workspace_id, policy_id, policy_version, granting_principal_id,
rule_key, scope_json, reason_ref, assurance, single_use,
issued_at, expires_at, revoked_at, consumed_at nullable
```

Exceptions cannot alter their scope after issue and are evaluated/revoked
server-side. Three divergences from the sketch this table started as are deliberate, and
`migrations/sqlite/000005_model_policy_exceptions.sql` records the reasoning:

- **`state` is not stored.** Usability is a question about an *instant* — derived from
  `revoked_at`, `consumed_at`, and `expires_at` — so a stored column would be wrong the moment
  the clock passed `expires_at` and would be a second answer to a question the domain's
  `state_at` already answers.
- **The scope is one serialized column, not two.** The "constrained value" and the
  "provider/model/task scope" are one `ExceptionScope` value whose fields are validated
  together, so splitting them would be two places for the stored form and the domain type to
  disagree. The same reasoning stores policy rules as `rules_json`.
- **`assurance` is stored rather than recomputed.** It records what the granting principal
  actually held, so an operator who stepped up approved *that* relaxation; re-deriving it from
  the rule later would let a change to the step-up policy rewrite what a past grant meant.

### Implemented evidence: conversations, runs, activity, and model calls (`BRN-004`)

`migrations/sqlite/000002_conversations_runs.sql` creates `conversations`,
`messages`, `agent_runs`, `agent_steps`, `run_activity_events`, and `model_calls`,
and raises the schema version to 2 with the minimum reader left at 1 because the
migration is purely additive. `jarvis_application::repository` defines the ports
(`RunRepository`, `ConversationRepository`, `ModelCallRepository`) and
`jarvis_infrastructure::storage::repositories` implements them over SQLite.

`migrations/sqlite/000003_idempotency.sql` raises the version to **3** — again with
the minimum reader at 1, because it is purely additive — and adds
`idempotency_records`, which the local control API's required `Idempotency-Key`
header needs. Two decisions there are load-bearing:

- The stored value is a **digest of the canonical request**, not the request body.
  Comparing digests is what answers "same key, different input?", and storing the body
  would retain caller text in a durable row for no benefit the comparison needs.
- The uniqueness key includes the **workspace**, the **operation**, and the **API
  major**, because the contract scopes idempotency to the authenticated principal,
  the resolved workspace, the client credential, the operation, and the API major.
  Without them one client's key could replay another client's run.

The record commits **with** the run it describes. `RunRepository::create_run_idempotent`
writes the run, its opening `run.received` event, and the key record in one
transaction, because the contract requires "acknowledged mutation state and
idempotency records are committed atomically". A separate claim followed by a create
leaves exactly the window that sentence closes, and the first implementation had it: a
replayed create reported a conflict instead of returning the original run.

`migrations/sqlite/000004_model_data_policy.sql` raises the version to **4** — the
minimum reader again stays at 1, because every existing table and column is untouched —
and adds the three tables `docs/contracts/model-data-policy.md` fixes:
`model_data_policies`, `model_policy_exceptions`, and `model_route_decisions`. Four
decisions there are load-bearing:

- **A policy version is immutable.** The row is insert-only with
  `UNIQUE (policy_id, version)`, so "changing rules creates a new version" is a constraint
  rather than a convention. An `UPDATE` would silently rewrite the rules a past route
  decision was made under, which is the one thing the contract's "historical records retain
  the policy version needed to explain a past decision" rule exists to prevent — and a
  stored `version` with no matching constraint is exactly the shape that invites one.
- **Rules are stored as JSON, not as columns.** `PolicyRules` is a typed struct with seven
  enums and four sets, so flattening it into twenty columns would create twenty places for
  the stored form and the domain type to disagree, with no single reader that could notice.
  One serialized value means one representation, and a read that cannot be reinterpreted is
  reported as `Corrupted` rather than as absence — otherwise a caller could create a
  replacement for a policy that is still there.
- **`route_decisions` is its own table rather than a column on `model_calls`.** One decision
  can serve a retry's later attempts, and the contract treats the considered candidates and
  their rejection reasons as an auditable artifact in its own right.
- **A decision stores its own copy of the policy reference.** Reading it through a live join
  to `model_data_policies` would make an archived version's decision unreadable, which is
  the case the historical-record rule is about.

The store reports a duplicate version as `storage.version_conflict` — the same typed
outcome the run state machine uses, because it is the same fact: someone advanced the
record since this caller read it. Assigning the next version inside the store would make a
concurrent update indistinguishable from a sequential one, and the contract requires
`resource.version_conflict` for the former. `load_active` orders by version descending, so
a workspace that archived and re-activated across several versions gets the newest rather
than whichever row was inserted first.

Three constraints are load-bearing rather than decorative:

- `agent_runs` refuses a **waiting** state whose dependency is unset and a
  **terminal** state whose completion instant is unset. Without them a row could
  read as waiting with nothing to wait for, or as finished with no instant, and
  both would look plausible to a reader. The port mirrors the rule through
  `RunWrite::is_consistent`, and there is a test for each direction of the
  mismatch.
- `UNIQUE(run_id, sequence)` on `run_activity_events` keeps "sequence increases by
  exactly one" true, so a retried append cannot place two events at one position.
- `UNIQUE(logical_call_id, attempt)` on `model_calls` makes a retry an *attempt* of
  one logical call rather than a new call, which is the distinction the
  retry-ownership rule depends on.

`RunRepository::transition` commits the state change and its activity event in
**one transaction** — the first required atomic use case in the storage
architecture — so a reader never sees an event describing a transition that is not
durable, or a transition with no event.

**A defect this slice found, and the reason the edge check does not live in the
SQL predicate.** The first implementation inferred the legality of a transition
from its `... AND state = ?` predicate, which only proves the run *was* in the
expected state — it does not prove the edge exists. That version **accepted
`Received -> Responding`**, an edge the architecture diagram does not contain, and
it passed every test until one asserted the refusal. The adapter now reads the
current row inside the transaction, orders its refusals exactly as the domain
does (**terminal, then version, then edge**) and asks
`RunState::can_transition_to` — the domain's own table — rather than duplicating
or approximating it. `an_edge_the_diagram_lacks_is_refused` is the test.

A second class of defect was found the same way, in the opposite direction:
constraint failures were mapped to `storage.query_failed`, reporting a
**caller-visible conflict** as a transport fault a blind retry might "fix". A
unique-constraint violation now maps to `storage.conflict` across runs,
conversations, messages, activity events, and model calls.

Storage-level outcomes are deliberately separate from domain errors: a missing row
and an illegal transition are different facts, so `RepositoryError` carries the
domain's refusal code through in `TransitionRefused { code }` instead of
flattening it. A run in another workspace is `NotFound`, never a forbidden result,
because the local control API requires the two to be indistinguishable. A stored
state or role the domain does not recognize is `Corrupted`, never "absent",
because treating it as absent would convert a migration problem into apparent data
loss.

**Still schema-only:** `agent_steps` has a migration and a schema but **no port or adapter**, so no
step is persisted — the `agent_steps` columns above have no writer. That is a real gap rather than a
mislabel: `BRN-005` built the run *state machine*, which is a different object, and the step record
arrives with tool-fabric work. `sessions`, `users`, `workspaces`, and `client_credentials` remain
schema-only — local single-owner enrollment still uses the Foundation credential file. No PostgreSQL
implementation exists (that is `PRD-001`), and the schema-version bump has not been exercised as an
upgrade from a populated version-1 database, which `docs/data/migrations.md` requires and belongs to
`PRD-009`.

**Corrected from an earlier note that said otherwise:** `model_data_policies`,
`model_policy_exceptions`, and `model_route_decisions` **are** created and written.
`000004_model_data_policy.sql` created the first and third and a placeholder second, and
`000005_model_policy_exceptions.sql` replaced that placeholder with the implemented shape;
`PolicyService` writes a policy version, `RunService::select_route` persists a route decision, and
the exception lifecycle has a port and an adapter. The paragraph that stood here claimed all three
"are not created, because routing and the data policy are `BRN-010`" long after `BRN-010` was
implemented — a claim the gate cannot see, because it checks links and identifiers rather than
whether a sentence is still true.

## Tools, Policy, and Approvals

### `tool_sources`

```text
id, workspace_id nullable, kind, source_key, version, publisher,
configuration_fingerprint, status, health, last_seen_at
```

Built-in sources can be global; grants and discovered availability remain
workspace/account scoped.

### `tool_definitions`

```text
id, canonical_tool_id, source_id, semantic_version, schema_fingerprint,
definition_json, effects_json, risk, status, created_at, superseded_at
UNIQUE(canonical_tool_id, source_id, schema_fingerprint)
```

### `tool_availability`

Maps definition/source to workspace, connector account/runtime/export context,
health, discovered metadata version, and last verified time. Availability is not
permission.

### `policy_decisions`

```text
id, workspace_id, principal_id, run_id, tool_call_id,
policy_version, decision, reason_codes_json, constraints_json,
input_fingerprint, created_at
```

### `tool_calls`

```text
id, workspace_id, run_id, step_id, runtime_call_ref,
tool_definition_id, source_id, principal_id, state, version,
arguments_ref, argument_fingerprint, idempotency_key,
policy_decision_id, approval_id, attempt_count,
provider_reference, result_ref, effect_summary, error_code,
created_at, reserved_at, started_at, completed_at
UNIQUE(workspace_id, tool_definition_id, idempotency_key)
```

The uniqueness scope may include connector account/operation after tool-specific
idempotency analysis.

### `approvals`

```text
id, workspace_id, requesting_principal_id, deciding_principal_id,
run_id, tool_call_id, state, version, action_fingerprint,
risk, effects_json, preview_ref, allowed_channels_json,
decision_channel, assurance, expires_at, decided_at, consumed_at,
created_at
```

One-shot consumption and tool reservation occur atomically.

## Memory, Entities, and Context

### `memories`

```text
id, workspace_id, subject_id nullable, memory_type, schema_version,
content_ref, canonical_text, confidence, importance, sensitivity,
confirmation_state, valid_from, valid_until,
created_at, updated_at, last_accessed_at, archived_at, deleted_at
```

### `memory_sources`

```text
memory_id, source_kind, source_id, source_timestamp, extraction_ref,
reliability, created_at
```

### `memory_relations`

Relations such as `supersedes`, `contradicts`, `derived_from`, and `reinforces`,
with source and confidence.

### `entities`

```text
id, workspace_id, entity_type, canonical_name, status, confidence,
created_at, updated_at, merged_into_id nullable
```

### `entity_aliases`, `entity_external_ids`, `entity_relations`, `memory_entities`

Aliases/external IDs are source/account scoped. Merge/split lineage is retained.

### `embeddings`

```text
id, workspace_id, owner_type, owner_id, model_provider,
model_id, model_revision, dimensions, content_hash,
vector_or_blob, created_at
UNIQUE(owner_type, owner_id, model_provider, model_id, content_hash)
```

PostgreSQL uses pgvector for the vector column/index. SQLite representation is a
researched adapter choice; no extension is assumed in this conceptual schema.

### `context_manifests`, `context_manifest_items`

Store policy/version, budgets, item references, reasons, rank components,
sensitivity, token estimates, truncation/exclusion summaries, and tool catalog
projection.

## Documents and Artifacts

### `documents`

```text
id, workspace_id, source_kind, source_id, title, media_type,
content_artifact_id, content_hash, sensitivity, status,
created_at, updated_at, deleted_at
```

### `document_chunks`

```text
id, workspace_id, document_id, sequence, locator_json,
text_ref, content_hash, token_count, created_at
UNIQUE(document_id, sequence)
```

### `artifacts`

```text
id, workspace_id, owner_principal_id, storage_backend, storage_key,
media_type, byte_length, sha256, sensitivity, retention_class,
encryption_profile, provenance_json, created_at, expires_at, deleted_at
```

## Connectors

### `connector_definitions`

Registry metadata for connector ID/version/manifest fingerprint/quality and
compatibility. No credential values.

### `connector_accounts`

```text
id, workspace_id, connector_id, external_tenant_id, external_account_id,
display_name, auth_strategy, credential_ref, granted_scopes_json,
status, health, config_version, config_json,
created_at, updated_at, last_verified_at
```

External IDs have composite uniqueness under connector and tenant as appropriate.

### `oauth_flows`

Short-lived state/PKCE/nonce records bound to principal, workspace, connector,
redirect target, expiry, and consumed state. Verifier/secret material uses a
protected reference or encrypted short-lived store.

### `sync_cursors`

```text
id, workspace_id, connector_account_id, resource_type,
cursor_version, cursor_ref, watermark_at, completeness,
last_attempt_at, last_success_at, error_code, backfill_state_json
UNIQUE(connector_account_id, resource_type)
```

### `webhook_subscriptions`, `webhook_deliveries`

Subscriptions track provider remote identity/state and secret reference.
Deliveries track scoped external delivery ID, signature outcome, raw artifact
reference under retention, received/processed state, and normalized event ID.

## Events, Jobs, and Workflows

### `events`

```text
id, workspace_id, event_type, schema_version, aggregate_type,
aggregate_id, aggregate_version, principal_id nullable,
correlation_id, causation_id, payload_ref, sensitivity,
occurred_at, recorded_at
```

Index workspace/type, aggregate/version, correlation/causation, and recorded
time. System events use the explicit system workspace and contain no tenant
payload.

### `event_outbox`

```text
id, workspace_id, event_id, destination_or_handler, state, attempt_count,
available_at, lease_owner, lease_token, lease_expires_at,
last_error_code, delivered_at, created_at
UNIQUE(workspace_id, event_id, destination_or_handler)
```

### `event_inbox`

```text
workspace_id, event_id, handler_id, handler_version, state, attempt_count,
result_ref, processed_at
PRIMARY KEY(workspace_id, event_id, handler_id, handler_version)
```

### `workflow_definitions`

```text
id, workspace_id nullable, key, version, definition_json,
schema_fingerprint, status, created_at
UNIQUE(workspace_id, key, version)
```

### `workflow_runs`

```text
id, workspace_id, definition_id, owner_principal_id, state, version,
input_ref, output_ref, correlation_id, current_wait_kind/ref,
deadline_at, created_at, started_at, updated_at, completed_at
```

### `workflow_steps`

Mirrors durable step fields with node key, dependency state, activity/tool/run
reference, retry/timeout/compensation, lease/fencing, and result/error.

### `scheduled_jobs`

```text
id, workspace_id, owner_principal_id, schedule_kind, schedule_expression,
timezone, next_fire_at, misfire_policy, overlap_policy, jitter_ms,
command_template_ref, policy_ref, status, version,
lease_owner, lease_token, lease_expires_at, last_fire_at
```

## Runtimes and Plugins

### `runtime_definitions`, `runtime_instances`

Track runtime ID/version/protocol/capabilities, install/provenance, process/remote
identity, status/health, restart/quarantine state, and last heartbeat.

### `runtime_run_bindings`

Maps JARVIS run/attempt to runtime instance/run/checkpoint, acknowledged sequence,
state, and timestamps.

### `plugin_installations`, `plugin_grants`

Track manifest/package hash/signature/publisher/source, compatibility, enabled/
quarantine state, config ref, requested capabilities, and separately approved
workspace grants.

## Voice and Notifications

### `voice_calls`

```text
id, workspace_id, owner_user_id nullable, acting_principal_id nullable, provider_id,
provider_call_id, provider_conversation_id, direction, state, version,
identity_assurance, reason_ref, related_run_or_workflow,
consent_policy_ref, idempotency_key, usage_json, estimated_cost,
transcript_artifact_id, audio_artifact_id, outcome_ref,
created_at, connected_at, completed_at
```

The configured inbound number/agent/route or outbound workflow resolves the
workspace server-side before this row is created. An unknown caller has a guest
or null acting principal and minimal assurance; workspace association records
ownership and retention but grants no access. A callback that cannot resolve a
configured route is rejected or retained only in a workspace-scoped security
quarantine, never as an unscoped canonical call.

### `voice_call_events`, `voice_turns`, `voice_callback_deliveries`

`voice_call_events` stores workspace/call/sequence, expected/new version, event
type, source, source delivery reference, correlation/causation, bounded payload
reference, and occurred/observed/recorded timestamps. Exactly one terminal event
is enforced per call.

`voice_turns` stores call/turn identity, state/version, finalized input/output
references, current model/tool/TTS bindings, latency/deadline fields, and terminal
outcome. Partial transcripts are ephemeral unless retention policy explicitly
permits a protected diagnostic artifact.

`voice_callback_deliveries` stores workspace, provider connection, scoped
delivery ID, signature/timestamp outcome, call binding, raw protected artifact
reference, received/processed state, and reconciliation result.

### `notifications`

Records channel, recipient reference, content artifact, urgency, quiet-hour
decision, state, idempotency, provider reference, and delivery/read timestamps.

## Audit and Operations

### `audit_logs`

Append-oriented actor/action/resource/policy/approval/outcome records with
correlation and safe metadata. Payloads and secrets are not copied here.

### `schema_migrations`, `application_locks`, `diagnostic_events`

Track migration checksums/status, scoped maintenance locks, and bounded safe
operational findings.

**`application_locks` is created but unused.** The table and its lease semantics are specified
here, and a `storage::lock` module once implemented them, but no code path in the product ever
acquired or released a row — so the table has no writer, and `docs/data/migrations.md`'s upgrade
runbook has been corrected to stop telling an operator to acquire one. Do not read this heading as
evidence that a maintenance lock is taken: the guard against two daemons sharing a profile is the
held file lock in `lifecycle::InstanceGuard`. A row-level lease is a distinct mechanism (a visible
holder and expiry, reclaimable after a crash) and is still unimplemented; a later slice should add
it with a caller attached rather than keep it in case.

## Critical Constraints and Indexes

- Foreign keys include workspace where practical to prevent cross-workspace
  association.
- Unique keys for idempotency, external webhook deliveries, event handling, and
  run/tool sequences.
- Partial indexes for pending approvals, runnable jobs, outbox deliveries,
  active runs, reauth connectors, and non-deleted memory.
- Check constraints for valid state/timestamp combinations.
- FTS indexes are scoped/joined through workspace-owned rows.
- Vector indexes/queries apply workspace filter at query time.
- Audit/outbox tables use retention/partition strategy in PostgreSQL after
  measured volume; SQLite uses bounded archival/vacuum policy.

Executable migrations must prove these invariants on both backends.