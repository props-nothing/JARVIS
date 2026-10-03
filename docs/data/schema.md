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

**Three columns here have no producer, and this list read as though they did.** `result_ref`,
`error_ref`, and `plan_summary_ref` are named above as ordinary members of the record, and a sweep of
the migrations against the Rust sources found **no reference to any of them anywhere in `crates/` or
`apps/`** — so every row carries `NULL` and a reader of this document would take them for
implemented. The contract's durable-run-record list does require "final response/artifact references
or normalized error", so two of the three are *owed* rather than unnecessary; they are absent because
each needs an artifact store, which does not exist (`result_ref` on `tool_call_records` is absent for
the same reason, and that note is recorded at the `tool_call_records` section). `plan_summary_ref`
needs a plan, and nothing writes one.

They are called out rather than deleted because the distinction matters: a column with no port is
`MEM-008`-style deferred work, while a column nothing references is a schema promise with no owner.
`error_code` is the one that *is* populated — `BRN-012` gave it a writer — which is why a failed run
reports its code and `error_ref` is still `NULL` beside it.

`runtime_id` and `runtime_version` name the runtime that *executed* the run and the build version it
was, so a resume can validate both against the runtime it is about to use
(`docs/architecture/agent-runtime.md`). They are written by the create path and read back by the
same port: before that, the `CreateRunRequest.runtime` field was required and validated and then
discarded, so both columns were referenced by no code at all and every row carried `NULL` while the
contract expected a value. Both are nullable because a row written before they had a writer has no
truthful value to give, and a row carrying **one** of the pair is reported as corruption rather than
as absent — half an identity cannot be validated against anything.

### `run_resume_states`

```text
workspace_id, run_id, approval_id, state_version, state_json, created_at
```

**Implemented (migration `000014_run_resume_states.sql`, schema version 14).** What a run parked on an
approval needs in order to continue: the batch of tool calls the model proposed, the observations of the
ones that already settled, the index of the one that is waiting, the turn, and the objective.
`(workspace_id, run_id)` is the primary key, so a run has **at most one** record and parking again
replaces it — a stale record would resume the wrong call. The record is written **before** the run is
parked and removed when the run leaves the wait; a run whose record cannot be written is failed rather than
parked. `state_json` is bounded by a `CHECK` as well as by the writer, and `state_version` lets a reader
refuse a record whose shape it does not know (reported as corruption, never guessed at). It is not
foreign-keyed to `agent_runs`: every statement scopes by `workspace_id`, and a cascading delete would
let a pruned run silently remove evidence. See `docs/architecture/agent-runtime.md`, "Resuming after
an approval".

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

  `usage_json` and `estimated_cost_microunits` are the last two columns in this section to be read,
  and the same rule applies to them (`BRN-058`): both were bound by `record_outcome` on every write
  and named by **no** `SELECT`, so a run's consumption was stored and unreachable. They are read
  together, from one decoder, because the cost is lifted out of the usage block so a ceiling check
  reads the block while a cost read reads the column — and if only one were selected, a run's summed
  cost could contradict the per-call values it was summed from. `load_run_calls` orders by
  `(started_at, attempt, id)` so the attempts of a run's several logical calls read in the order they
  happened, and `id` is the tie-break because the order must be total — a run can make two calls in
  the same instant, and a non-total order is how a summing read loses a row.
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
rules_schema_version, rules_json, layers_json nullable, created_by,
created_at, activated_at, superseded_at
UNIQUE(workspace_id, id, version)
```

Policy versions are immutable. One active version per policy identity/workspace
is selected through an optimistic transition.

`layers_json` (added by `000011`) records the **contributions** that produced `rules_json`: the layer,
the rules it supplied, and the stored version it came from. It exists because the contract's
`GET /api/v1/model-data-policy` requires the response to return "source layers", and the merge that
decides them happens inside `PolicyService::put` — so before this column the reason for a rule was
computed and dropped in the same step. It is nullable and **not back-filled**: a row written before
provenance was kept genuinely has none, and defaulting it to `["workspace"]` would attribute a
narrowing to a layer nobody observed. A reader treats NULL as "no known contributor", which is why
the wire field is `default`-ed rather than required.

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

**As built (`000009`), named `tool_call_records`**, and the name difference is the one
deliberate divergence. The design above is named for the *call* and carries `step_id`,
`runtime_call_ref`, `arguments_ref`, and `result_ref` — every one of which needs an artifact store,
a step writer, or an external runtime, none of which exists. What was built is what `TLS-006`'s
domain module actually owns:

```text
id, call_id, workspace_id, principal_id, run_id,
tool_identity_json, idempotency_key, attempt, operation,
state, version, dispatched_at, outcome, no_effect_confirmed,
created_at, updated_at
UNIQUE(workspace_id, principal_id, tool_identity_json, idempotency_key)
```

- **One row per reservation key, not per attempt.** The unique index is
  `(workspace_id, principal_id, tool_identity_json, idempotency_key)`, so a second attempt at the same
  key is *refused by the reservation* rather than stored beside the first — and the `attempt` column
  records how many times that one row's call was tried, not which row it is. An earlier version of this
  note claimed "an invocation that is retried has several rows sharing one reservation key, and the
  attempt number is what distinguishes them", which the index contradicts: `attempt` is not a key
  column, so two rows cannot share a key at all. `a_second_attempt_at_one_reservation_key_is_refused…`
  asserts the refusal with an `attempt = 2` row, and the domain ledger is a map keyed on the whole
  reservation key, so the same fact holds in memory. The table is named `tool_call_records` because it
  records a reservation and its outcome, and the reservation — not the attempt — is the unit the unique
  index deduplicates.
- **The unique index is over the serialized identity text**, so it only holds because the domain's
  serializer is deterministic for a struct of fields. A representation whose serialization could
  reorder — a map, or a set field — would make the index silently stop deduplicating while still
  existing. The adapter asserts the determinism rather than assuming it.
- **`principal_id` is in the key**, unlike the design's `UNIQUE(workspace_id, tool_definition_id,
  idempotency_key)`. A key without it would let one principal's retry collide with another's fresh
  call, and the consequence here is a second side effect rather than a leaked identifier.
- **`dispatched_at` is stored rather than derived from `state`.** This is the `TLS-006` defect in
  column form: `may_have_effected()` answering "no" for a call that reached `EXECUTING` and then
  `FAILED` is exactly what retries a call whose effect may have landed. The reader refuses a state
  that requires a dispatch without one.
- **`outcome` is `NULL` for an unknown outcome**, which is a different fact from "it failed". A
  terminal row with no outcome is refused rather than defaulted, because defaulting it would let an
  unknown outcome read as a recorded failure a caller may retry.
- **The design's `approval_id` is not here.** The link exists in the other direction —
  `approvals.tool_call_id`, added by `000008` — and a duplicate foreign key on this side would be a
  second copy of one relationship that could disagree with the first.

Nothing writes a `tool_calls` row: there is still no executor, and the controller refuses a tool
intent with `run.tools_not_implemented`. The adapter is exercised by contract tests over a real
migrated database.

### `approvals`

```text
id, workspace_id, requesting_principal_id, deciding_principal_id,
run_id, tool_call_id, state, version, action_fingerprint,
risk, effects_json, preview_ref, allowed_channels_json,
decision_channel, assurance, expires_at, decided_at, consumed_at,
created_at
```

One-shot consumption and tool reservation occur atomically.

**As built (`000008`, plus `000010`'s column)**, which differs from the design above in five ways the
implementation decided:

```text
id, workspace_id, requesting_principal_id, run_id, tool_call_id,
tool_identity_json, action_fingerprint, risk, effects_json, summary,
preview_json, allowed_channels_json, expires_at, scope, state, version,
decided_by, decided_via, decided_assurance, decided_at, created_at, updated_at
```

**Two of the three indexes `000008` creates index a query that does not exist yet, and the migration's
own comment reads as though they do.** It says each index covers "a query the port actually has" and
names three: `pending_in` (exists), `decided_by` ("answers *what did I approve?*"), and a lookup "every
tool call performs — *is there an approval for this call?*". The first two have port methods; the third
has **neither a method nor a caller**, and `tool_call_id` is selected and rendered but never used as a
filter anywhere in the workspace. `decided_by`'s method exists and is called from nothing but the
adapter's own tests.

An index is cheap and a missing one is a scan, so this is not harm — but the comment is a claim, and a
reader who trusts it concludes the tool-call lookup is implemented. The honest state: one index for the
listing, one for a method that exists with no caller, and one for a query nobody has written. Each is the
right index **for the query it was designed around**; what is missing is the queries, not the indexes.

- **`tool_identity_json` rather than a `tool_definition_id`** and separate source columns. An approval
  binds the whole identity tuple — capability, source, provenance, schema fingerprint — and the domain
  type's serialization is the canonical spelling of it, already round-trip tested. Splitting it would
  create a second definition of what the identity is, and the first thing to drift would be the
  fingerprint's algorithm prefix.
- **`deciding_principal_id` is `decided_by` plus `decided_via`**, and **`decided_assurance`** was added by
  `000010`. The contract lists `allowed_channels` so a decision can be checked against them, and a record
  that did not store the channel used would make the check one-directional — JARVIS could refuse a
  disallowed channel going in and never say, afterwards, which one a decision came from. The assurance is
  the contract's Audit requirement, and it is a property of the **record** rather than of one transition:
  the first implementation stored it from each transition's actor, so a consumption's actor — which carries
  no assurance — wrote `NULL` over the value the decision had established. `NULL` there means **not
  recorded** (the state of a decision taken before the column existed) and is deliberately distinct from
  `standard`; the reader refuses an unknown spelling as corruption rather than mapping it to a level. The
  level a decision **requires** is deliberately not a column: it is derived from the record's own `risk`
  (`RequiredAssurance::required_for`), so "critical actions default to step-up" has one owner rather than
  two values that must agree. A per-record override still has no producer and is named in the contract.
- **`summary` and `preview_json` are stored verbatim**, because they are the *record* of what the user
  was shown and agreed to. Rebuilding them from the tool would show a preview the user never saw, whose
  definition may since have changed. **Both are read back through their constructors**, so a stored row
  whose summary or preview item the domain would refuse is `storage.row_corrupted` rather than a value
  carried forward — these two are rendered inside the consent prompt, so the reader is a path an
  unvalidated value could take into that prompt. (Before `BRN-060`, `summary` was a bare `String` read with
  no check at all, and `preview_json` was decoded with a derived item deserializer.)
- **The decision's note is in `approval_transitions.actor_json`, not in a column of `approvals`.** A
  decision's `comment` and a cancellation's `reason` explain *that* transition — the only two a human
  authors — so they belong to the step rather than to the record, and a column would have to answer what
  it holds once a later consumption or expiry overwrote it. The trail is read oldest first by
  `(occurred_at, id)`, and the note is surfaced by the **detail** route rather than a listing, which would
  otherwise be one trail read per row.
- **`state` has no default.** A row whose state was unset would be one whose lifecycle position nobody
  chose, and defaulting to `pending` is the fail-open direction — a pending approval is one a later
  pass may decide. `NOT NULL` with no default makes that a write error instead of a plausible row.

### `approval_transitions`

```text
id, approval_id, workspace_id, from_state, to_state,
prior_version, version, actor_kind, actor_json, occurred_at
```

One row per applied transition, written **in the same transaction** as the state change, so the trail
cannot disagree with the state it describes — the architecture's "persist state before publishing an
event that claims the transition happened" rule applied to a decision. `actor_kind` is separate from
`actor_json` so a query can count decisions without parsing, and it is what makes "time is not a
decider" visible: an `expired` actor has no principal.

**As built (`jarvis_infrastructure::storage::approval_repository`)**, the two reading rules the adapter
adds on top of the schema, because both are decisions the columns alone do not state:

- **A stored state is *walked*, not assigned.** The reader rebuilds the record through the domain's
  request path and then drives the domain's own transition table from `pending` to the state the row
  claims. A row claiming a state the table cannot reach — `pending -> consumed`, which would be a spent
  approval nobody ever granted — is `storage.row_corrupted` rather than a reconstruction. The version is
  derived by that walk and cross-checked against the stored `version`, so a row whose state and version
  disagree is refused: its transition count is not one its state explains.
- **A decision instant is read, never invented.** `decided_at` is the transition's own instant for a
  decision state and the row's `updated_at` for a non-decision one, and a decision row with no recorded
  instant is corruption rather than a default — `DurableApproval::apply` always writes one for a
  decision, so its absence means the row was not written by the domain.
- An approval in another workspace is `storage.not_found`, **indistinguishable from one that does not
  exist**, following the rule the local control API states for runs.
- **The fingerprint is re-validated on read.** `action_fingerprint` is stored as the contract's
  `sha256:<hex>` text and read back through `ActionDigest::parse`, so a row whose column holds something
  that is not a digest — a bare hash, another algorithm, uppercase hex — is `storage.row_corrupted`
  rather than a value that is carried into a comparison and silently never matches. That is the same
  direction the state walk takes: an uninterpretable stored value is corruption, not data loss to
  tolerate, because the alternative is a check that quietly stops applying.
- **The pending read's channel filter is inside the query, and the bound is a probe.** `approved`
  channels live in a JSON array, and `pending_in` tests membership with
  `EXISTS (SELECT 1 FROM json_each(...) WHERE value = ?)` **before** the `LIMIT` — so the page bound
  applies to the rows the caller can actually decide. Filtering after the `LIMIT` spends the page on
  rows the caller cannot act on and returns a short list, and since this listing serves no cursor a
  client that receives a short list concludes there is nothing left to decide. The statement reads
  **one row more than the bound** so `bounded` is an observation rather than an inference from
  `len() == limit`, and it orders by `(expires_at, id)`: soonest deadline first because that is the
  order an operator works in, and the identifier as a tie-break because an order that is not total can
  show one row twice and hide another. `json_each` needs the JSON1 extension, which the bundled SQLite
  provides (probed against the pinned crate: 3.51.3); a build without it fails the query loudly rather
  than silently dropping the channel rule.

### `tool_call_transitions`

```text
id, record_id, workspace_id, from_state, to_state,
prior_version, version, outcome, occurred_at
```

One row per applied ledger transition, written **in the same transaction** as the state change. For a
tool call the trail is load-bearing rather than decorative: it is the durable answer to "was this call
ever dispatched", which is the single fact a later retry decision turns on. A state change that
committed without its trail row would leave the state and the audit of it disagreeing.

**As built (`jarvis_infrastructure::storage::tool_call_repository`)**, three rules the adapter adds
because the columns alone do not state them:

- **The reservation is one statement.** `reserve` performs a single `INSERT` against the unique index
  and takes its verdict from that statement's outcome — a unique violation means the row that won is
  read and classified *after* the constraint fired, when it is durable. There is no read-then-decide
  window, which is what makes the reservation atomic **across processes** rather than only within one
  daemon's memory. The tests reserve from two separate connections on one file database, because two
  repositories over one pool would pass for a read-then-write adapter.
- **A stored row is *validated*, not replayed**, and that difference from `approvals` is forced by the
  state machine. An approval's version is derivable from its final state; a ledger's is not, because
  `REQUESTED -> VALIDATED -> RESERVED -> EXECUTING -> SUCCEEDED` and the same path through
  `WAITING_APPROVAL` both end `SUCCEEDED` at different versions. So the reader asserts the invariants
  the rows must satisfy instead: a non-zero attempt, a version at least the first, a terminal row with
  an outcome, and **a state that requires a dispatch recording one**.
- **A row in another workspace is `storage.not_found`**, while the reconciliation scan is deliberately
  **unscoped** — a scan filtered by workspace would leave other workspaces' stranded effects stranded
  with no symptom.

**Startup recovery (`jarvis_application::tool_recovery`, run by `jarvisd` before readiness)** settles
calls a restart interrupted, in **two directions**. A call that reached the provider and then lost its
daemon stayed `EXECUTING`, and the next attempt's reservation answered `InFlight` — telling a caller to
wait on a process that is gone. The pass moves it to `RECONCILING`, so a duplicate is reported as
`Unsettled` and a caller is told to reconcile rather than wait. A call that never dispatched is a
**second** case with the same harm and a different remedy: nothing reached the provider, so it cannot
have effected, and leaving it reserved means its dead holder keeps the key and every later retry waits on
it for ever — so the pass ends it `CANCELLED` (with the `tool.cancelled` outcome a terminal row
requires). Two rules there are consequences of the storage shape:

- **The scan a caller reports is not the scan the pass pages.** `possibly_effecting` returns
  `executing` **and** `reconciling` rows with no outcome — the outstanding work. Paging it would re-read
  the pass's own output, since settling an `EXECUTING` row produces a `RECONCILING` row that the same
  predicate returns; with the ordering tied on `updated_at`, a later page can hold only settled rows and
  the rest become unreachable. `awaiting_conversion` returns every non-terminal state **except
  `reconciling`**, so the set strictly shrinks and the paging terminates. **That "except" is exactly one
  state, and a narrower predicate is a defect rather than a tuning choice**: the pass's classification
  answers for six non-terminal states, and an earlier version selected `executing` alone — so a stranded
  **pre-dispatch** reservation was never settled, its dead holder kept the unique-index entry, and every
  later retry of that key was answered `InFlight`, i.e. *wait on a process that is gone*.
  `the_conversion_scan_covers_every_state_the_pass_can_settle` asserts the predicate against the domain's
  own state set, because the SQL list and the Rust table compile independently.
- **`bounded` comes from a probe row, not from `len() == limit`.** The adapter reads one row more than the
  page size, so "there is more" is distinguishable from "that was all" — and this bound hides the
  **newest** rows, which is why the distinction decides whether recovery can finish.


## Memory, Entities, and Context

### `memories`

```text
id, workspace_id, subject_id nullable, memory_type, schema_version,
content_ref, canonical_text, confidence, importance, sensitivity,
confirmation_state, valid_from, valid_until,
created_at, updated_at, last_accessed_at, archived_at, deleted_at
```

**Implemented subset (migration `000013_memories.sql`, schema version 13):** `id`, `workspace_id`,
`memory_class`, `canonical_text`, `dedupe_key`, `sensitivity`, `source_kind`, `source_principal_id`,
`created_at`, `updated_at`. `memory_class` is constrained to `preference`/`semantic` and `source_kind` to
`user_request`; `(workspace_id, dedupe_key)` is unique. The remaining columns above are the conceptual target
and are added by the migration that gives them a writer and a reader, not before. Forgetting deletes the row.

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

**`diagnostic_events` has no production writer either, and the reason is a genuine design question
rather than an omission.** The migration describes it as "bounded, safe operational findings produced
by doctor and startup checks", and there is no insert outside `#[cfg(test)]`. Two things have to be
decided before it can have one, and neither is answerable by looking at the table:

- **Where the finding is produced.** `jarvis doctor` runs in the **CLI** process
  (`apps/jarvis-cli`, over `jarvis_infrastructure::diagnostics`), while the table is opened by the
  **daemon**. A CLI finding is written into the same SQLite file the daemon owns, which is the same
  ownership question `InstanceGuard` answers for the file lock — so "the CLI writes a row" is a
  decision about concurrent writers, not a missing `INSERT`.
- **Whether a passing check belongs there.** Its severity constraint is
  `CHECK (severity IN ('info', 'warning', 'error'))` — `Ok` is deliberately absent, so the table is
  for findings worth keeping, not for a run's output. That is a reasonable design, and it means
  "record the doctor run" is not what this table is for.

So this is named rather than wired: the checks exist, the table exists, and what is missing is a
decision about which process records which findings and at what severity. Asserting the heading as
though a writer existed would be the `BRN-058` defect in the other direction — a reader concluding a
capability exists because a document names it.

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