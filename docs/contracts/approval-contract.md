# Approval Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Purpose

Approval records prove that an authenticated principal authorized one exact
action or a narrowly defined standing rule. They are not free-form chat messages.

## Approval Request

```json
{
  "approval_id": "019...",
  "workspace_id": "019...",
  "requesting_principal_id": "019...",
  "run_id": "019...",
  "tool_call_id": "019...",
  "tool_id": "email.send@1",
  "action_fingerprint": "sha256:...",
  "risk": "high",
  "effects": ["external_communication", "write"],
  "summary": "Send one email to peter@example.com",
  "preview": {},
  "allowed_channels": ["cli", "desktop"],
  "expires_at": "2026-09-20T12:10:00Z",
  "state": "pending"
}
```

Preview is schema-defined, bounded, redacted, and sufficient for informed
consent. Hidden attachment/body/target changes are prohibited.

## Action Fingerprint

Compute over a versioned object containing:

```text
fingerprint format version
principal and workspace
tool canonical ID, source identity, schema fingerprint
normalized arguments
material content/artifact hashes
target connector account/resources
effects and constraints
logical idempotency key
```

Use a researched deterministic JSON canonicalization such as RFC 8785 and
SHA-256, encoded with an explicit algorithm prefix. The implementation must
round-trip test across Rust and any client that previews/verifies fingerprints.

## States

```text
PENDING
APPROVED
REJECTED
EXPIRED
CANCELLED
CONSUMED
INVALIDATED
```

Terminal decisions are immutable. A one-shot approval is consumed atomically
when the exact tool call is reserved. Reuse or changed fingerprint fails.

## Decision

```json
{
  "decision": "approve",
  "expected_version": 3,
  "action_fingerprint": "sha256:...",
  "comment": null,
  "remember": false
}
```

`remember` is optional and means "always allow this" — see [Standing approvals](#standing-approvals-always-allow). The server derives deciding principal, device/session, authentication assurance,
and time. A client cannot assert them in the body.

## Approval API

Authenticated product clients use:

```text
GET  /api/v1/approvals
GET  /api/v1/approvals/standing
GET  /api/v1/approvals/{approval_id}
POST /api/v1/approvals/{approval_id}/decide
POST /api/v1/approvals/{approval_id}/cancel
POST /api/v1/approval-grants/{grant_id}/revoke
```

### List and Detail

List filters are schema-defined: state, risk, effect, requesting run/tool, and
created/expiry time. Results use opaque workspace/principal/query-bound cursors,
stable ordering, and a bounded page size. The server exposes only approvals the
authenticated principal may inspect or decide. A foreign-workspace record is
indistinguishable from a missing record.

**On that cursor wording, because two of the three bindings cannot both hold here.** A cursor binds the
*query* it was produced by, and this listing's query is bound by **workspace** and **channel**; it is
deliberately **not** principal-scoped (an operator view must show prompts waiting on somebody else, so a
listing narrowed to one principal would hide work). So "principal-bound" is not what this surface
implements, and the binding that actually matters is the pair the query filters on. The **workspace**
half needs no cursor field: it comes from the authenticated context on every request, so a cursor carried
into another workspace names a position in *that* workspace's own filtered rows rather than reaching
across — the workspace filter is applied unconditionally, which is strictly stronger than binding it into
the value. The **channel** half is carried in the cursor, because it is the caller's own choice rather
than the authenticated identity, and replaying a cursor minted for one channel against another is refused
(`request.invalid_cursor`) rather than answered with an empty page — the empty page *is* the disclosure of what that channel
excludes. When a principal-scoped listing exists, its cursor must carry the principal for the same reason
the channel is carried here.

**Every narrowing the query applies is carried in the cursor, and `risk` was the one that was not.**
A cursor minted under `?risk=critical` records the last *critical* row's `(expires_at, id)`; replayed
against `?risk=high` the `>` comparison would skip every high row expiring before that instant — rows the
caller asked for and would never see, which is the "a skipped approval is a prompt nobody decides" harm
the keyset bound exists to prevent, arriving through the *filter* rather than through an offset. So the
cursor carries the narrow it was minted under and a mismatch is `request.invalid_cursor`, exactly as for
the channel. A cursor with **no** recorded narrow is tolerated: it names an un-narrowed superset position,
so no row a narrower view wanted can fall behind it. The value stays opaque and versioned; the risk
segment is additive, and a cursor this build issued before it decodes as un-narrowed rather than as
invalid, because such a page genuinely was.

Detail returns the immutable action fingerprint inputs needed for informed
review as a bounded, schema-defined, redacted preview plus request/version/state,
requesting identity, tool source/schema identity, effects/risk, allowed channels,
expiry, prior decision metadata, and related run. It never returns hidden tool
arguments, credentials, or content excluded from the fingerprint.

**Implemented on the wire**: `ApprovalView` carries the capability as `tool_id`, the source as
`tool_source_kind`/`tool_source_owner`/`tool_source_version`, and the schema as `schema_fingerprint` —
**four fields rather than one joined tuple**, so a reviewer can see which dimension changed rather than
rendering a string they cannot take apart. That is the contract's "tool source/schema identity" made
literal, and it is what lets a client detect the `ACC-024` case: the tool behind a name was replaced, so
the approval on record is for a different implementation.

### Decide

`decide` requires `Idempotency-Key` and the decision body above. Allowed values
are `approve` and `reject`. The server:

1. authenticates the current session/device/service client;
2. resolves workspace and decision authority server-side;
3. checks current channel and assurance against the approval request/policy;
4. expires stale requests using the authoritative clock;
5. compares `expected_version` and exact `action_fingerprint`;
6. persists the immutable decision and outbox/resume signal atomically;
7. returns the approval state, not a claim that the side effect completed.

Same-key/same-request retry returns the original decision. Same key with changed
decision, fingerprint, or comment is `idempotency.conflict`. Concurrent opposite
decisions allow one optimistic transition; the loser receives
`approval.version_conflict` and the safe current state.

**The `comment` is stored, and it is stored on the decision's own transition** — `ApprovalActor::Decided`
carries a bounded `DecisionNote`, and a cancellation carries its `reason` the same way. It sits on the
transition rather than in a column because it explains *that* step: a decision is the only transition a
human authors, and a later consumption or expiry has no note, which is why the field is `None` rather
than empty. The bound and the control-character rule are enforced by the type's own constructor *and* by
its deserializer, so a value that arrived over the wire is held to the same rule as one this code built.
It is returned by the **detail** route and omitted from a listing: reading it means reading the trail, and
a listing that did so would be one trail read per row. An absent note is omitted rather than rendered as
an empty one, because "nobody wrote a comment" and "somebody wrote nothing" are different answers — the
second is refused at construction.

An approved one-shot action is still revalidated at reservation/invocation and
consumed atomically with the exact tool-call reservation. Approval never grants
general tool access.

### Standing approvals ("always allow")

An agent that asks about every call to a tool the user trusts is not usable, so a decision may be
remembered. `"remember": true` on an **approve** turns the approval into a *standing* one: it covers any later
call to the same tool identity by the same principal in the same workspace — not just the reviewed action — and
is **not spent by use**. It is the one place an approval is a pattern rather than an exact action, and it is
bounded so it cannot become a blank cheque:

- **Only for what a person can sensibly pre-authorize.** Effects limited to `read_only` and `write`, risk at most
  `moderate`, recorded on the approval when it was raised and never taken from the request. Anything that
  communicates externally, destroys, executes code, moves money, escalates privilege or acts physically — and
  anything of `high` or `critical` risk — always asks. A request to remember one is refused with
  `approval.standing_not_allowed` and **nothing is decided**: quietly downgrading to a one-time approval would
  let a user who asked for "always" find out by being prompted again. `remember` on a rejection is refused too.
- **Expiring.** Seven days from the decision (`STANDING_APPROVAL_WINDOW_MS`), after which it authorizes nothing.
- **Still inside the grant.** Policy matches a standing approval only after deny rules, the grant and its effect,
  risk and sensitivity ceilings have been satisfied; it can raise an `ask` to an `allow` and nothing more.
- **Visible and revocable.** `GET /api/v1/approvals/standing` lists the permissions in force (each row carries the
  `approval_id` and `version` that `cancel` needs), and cancelling an approved approval — the `approved ->
  cancelled` edge — withdraws it before the next policy check.

### Cancel and Revoke

The requesting principal or authorized policy/operator can cancel a pending or
approved-but-unconsumed request using `expected_version`, reason code, and
`Idempotency-Key`. Consumed/expired/rejected/cancelled requests are immutable;
idempotent repeats return current state.

Standing-grant revocation is a separate command bound to grant ID/version,
workspace, revoking principal/assurance, reason, and idempotency key. Revocation
commits before future policy checks and invalidates queued approvals/actions that
depend solely on that grant. It cannot erase historical audit.

### Expiry, Events, and Resume

Expiry is evaluated on every read/decision/reservation and by a durable expiry
worker. Exactly one state transition/outbox event wins. Approval activity events
are safe summaries; sensitive preview content remains referenced under policy.

A read past the deadline **records** the lapse rather than merely hiding the row:
a refusal that left the record `pending` would leave the prompt in every later
listing. The listing therefore expires the lapsed rows in its page and re-reads
until none remain, instead of filtering them out — a filtered page comes back
**short**, and on this surface a short page is how a client concludes there is
nothing left to decide.

Waiting runs/workflows resume from the durable decision event. A client
disconnect, duplicate event, or daemon restart cannot consume twice. Decision
responses may be `200`/`202` according to generated OpenAPI, but execution result
is queried through the related run/tool resource.

### Stable Errors

```text
approval.not_found
approval.scope_denied
approval.channel_not_allowed
approval.assurance_insufficient
approval.expired
approval.version_conflict
approval.fingerprint_mismatch
approval.already_consumed
approval.state_conflict
approval.grant_not_found
approval.grant_revoked
```

Errors use the common envelope and disclose no foreign approval, hidden target,
or sensitive preview. Authentication happens before lookup/body parsing where
the HTTP stack permits.

### CLI and Other Channels

The CLI is a thin API client with `approvals list`, `approvals show`,
`approvals approve`, `approvals reject`, `approvals cancel`, and grant-revoke
commands. Desktop/mobile render the same server preview/decision contract. Voice
uses the same endpoint/application command only after the voice call/session has
the policy-required assurance; transcribed channel text is never trusted
decision metadata.

## Standing Grants

"Always allow" creates a separate policy/grant proposal; it does not mutate a
one-shot approval. Standing grants define exact tool/source, resource/account,
argument constraints, effects/risk ceiling, channel/environment, expiry, budget,
and revocation. Critical actions may prohibit standing grants entirely.

## Voice Approval

Voice is permitted only when policy names it and the call has sufficient current
identity assurance. Critical actions default to step-up in CLI/desktop/mobile.
Transcribed "yes" alone is not universal approval.

## Invalidations

Pending approvals invalidate on:

- expiry, run/tool cancellation, or workspace/principal revocation;
- changed tool source/schema/effects;
- changed normalized arguments/material content;
- connector account or target resource change;
- policy version requiring reevaluation;
- runtime/plugin replacement;
- security kill switch.

## Audit and Privacy

Record request/decision/consumption identities, assurance, channel, policy,
fingerprint, safe preview hash/summary, timestamps, and outcome. Avoid storing
full sensitive content in the approval table; reference protected artifacts.

**Implemented for the decision**: `decided_by`, `decided_via`, and `decided_assurance` are stored as their
own columns and reported on the wire, so an operator can ask which decisions were made by a stepped-up
caller. Two properties are deliberate. The assurance is the value the **server** resolved from the
authenticated request — `ApprovalView` has no field for it in a decision body, so a caller cannot state
how strong its own decision looked. And it is a property of the **record** rather than of one transition:
recording it only inside the transition actor meant a later consumption or invalidation overwrote it, and a
consumed approval lost an assurance its own audit row still held. `NULL` means **not recorded** — the state
of a decision taken before the column existed — and is distinct from `standard`, because "we do not know"
and "we know it was ordinary" are different answers; a decision row read with no assurance is corruption
rather than a defaulted level.

Not yet stored: the policy version in force at the decision, and a preview hash rather than the preview
itself (the record stores the bounded preview verbatim, because it is the record of what the user was shown).

**The summary and the preview are validated on the way in, not only where the request is built.** Both are
rendered inside the consent prompt, and the preview is additionally read back on every load, so the reader
is a path an unvalidated value could take into that prompt. `ApprovalSummary` and `PreviewItem` are types
with validating constructors and hand-written `Deserialize` impls, and the SQLite reader builds both through
those constructors — so a stored row whose summary or preview item the constructor would refuse is
`storage.row_corrupted` rather than a value carried forward. The summary was previously a bare `String`
whose `MAX_SUMMARY_BYTES` bound was checked **nowhere in production** (`is_usable_summary` was referenced
only by its own tests), and `PreviewItem`'s fields were `pub`, so an unvalidated half could also be assigned
after a valid one had been constructed. Recorded in `BRN-060`.

## Tests

- concurrent approve/reject;
- stale expected version;
- changed recipient/body/attachment/account;
- expiry and daemon restart;
- consume exactly once under duplicate call submission;
- wrong principal/workspace/channel/**assurance** — the channel half by
  `a_decision_on_a_channel_the_request_excludes_is_refused_by_name`, the assurance half by
  `a_critical_action_cannot_be_decided_by_an_ordinary_session` (service) and
  `a_critical_action_is_forbidden_to_an_ordinary_session_over_the_wire` (wire, asserting the
  contract's own code and that the refused decision left the record pending at version 1);
- grant revocation and policy update;
- preview redaction and fingerprint cross-language vectors.
- list pagination/filter/cursor scope and foreign-workspace non-disclosure;
- API/CLI idempotency, optimistic decision race, cancel, grant revoke, and
  durable resume after disconnect/restart;
- server-derived channel/device/assurance and forged body metadata rejection;
- generated OpenAPI/client/golden fixture drift.

## Implementation Status

**Implemented**: the record and its lifecycle over the wire. `jarvis_application::approval_service`
owns list, read, decide, and cancel; `jarvis_protocol::approval` owns the wire shapes;
`jarvis_infrastructure::http::approval` serves four of the routes below; and `jarvisd` composes the
service over the daemon's own pool. `tests/e2e/approval-journey.mjs` proves the composition root — the
layer handler tests, which build their own `ApiState`, are structurally blind to.

What that covers, against the state machine: `PENDING`, `APPROVED`, `REJECTED`, `CANCELLED`, and the
`EXPIRED` transition a lapsed **read** records. The server derives the deciding principal and channel from
the authenticated request and the wire type has **no field for either**, so a client cannot assert
them. A repeated decision is idempotent (`applied: false`) rather than an error, and a fingerprint that
does not match the approved one is refused before the version, because a re-approval cannot fix a
digest that still will not match.

**Expiry is evaluated on every read path, not only on a decision.** `list` and `read` both take an
instant and **record** a lapse they find, so the two surfaces that *show* a prompt cannot report a
record the daemon then refuses as expired. The listing expires the lapsed rows in its page and re-reads
rather than filtering them out, for the same reason the channel predicate runs inside the query: a
short page is how a client concludes there is nothing left to decide. `tests/e2e/approval-journey.mjs`
seeds one live row and one already past its deadline, so the count in the listing is itself the
assertion. The contract's per-read rule was for several rounds satisfied only by `decide`.

**The listing's page bound applies to the rows the caller can decide, and that is a correctness
requirement rather than an optimisation.** `allowed_channels` is a JSON array the store tests with
`json_each` **in the query**, before the `LIMIT`. An earlier version applied the bound to the whole
workspace and filtered by channel afterwards, so a page could be spent on rows the caller could not
act on and come back short — and on a surface that serves no cursor, a client that receives a short
list concludes there is nothing left to decide and does nothing. The store also reads **one row more
than the bound** so `bounded` is observed rather than inferred from `len() == limit`, and orders by
`(expires_at, id)` so a page boundary inside a deadline tie cannot show one row twice and hide
another.

**Routes served**: `GET /api/v1/approvals`, `GET /api/v1/approvals/{approval_id}`,
`POST /api/v1/approvals/{approval_id}/decide`, `POST /api/v1/approvals/{approval_id}/cancel`.

**CLI served**: `jarvis approvals list|show|approve|reject|cancel`. Each is a named subcommand, so a
bare `jarvis approvals` can never decide anything — the same safety property `runs` and `install`
have. `approve` and `reject` require **both** `--fingerprint` and `--version` as arguments: the daemon
compares the digest it holds against the one the client states, so a decision naming none would be a
decision about *something*, and a default for either would let a caller decide an action it never
reviewed. The CLI prints the daemon's own body in both directions — its state on success so the two
cannot disagree, and its envelope on a refusal because the stable code is what an operator acts on.

`approvals list` carries `--limit`, `--risk`, and `--cursor`. The last two are the client half of the
listing's own features: `--risk` narrows to one contract level (a **closed set** parsed on the client,
so `--risk severe` is a usage error rather than a `400` from a round trip), and `--cursor` sends back
the `next_cursor` the daemon printed. Without `--cursor` the daemon's own cursor was a value a client
could read and not use — the same read-but-unusable dead end the cursor was introduced on the daemon
side to close, one layer out. Both are **omitted** when unset rather than sent as placeholders, so a
bare `list` is the un-narrowed first page.

**Not implemented, each named rather than implied:**

- **`POST /api/v1/approval-grants/{grant_id}/revoke`.** A standing grant is a *separate* record from a
  one-shot approval, and no standing-grant store exists — `policy_service` serves
  model-data-policy exception grants, which are a different concept with their own lifecycle and
  codes. Serving this route against the wrong record would be worse than not serving it.
- **Action fingerprint computation.** Implemented. `jarvis_domain::tool::canonical` produces the
  **RFC 8785** canonical form of a versioned envelope and `jarvis_infrastructure::tool_fingerprint`
  hashes it with SHA-256 into a `sha256:<hex>` digest; the evidence note is
  [rfc8785-canonicalization.md](../research/integrations/rfc8785-canonicalization.md). One limitation is
  deliberate rather than missing: the fingerprinted document is JARVIS's **own** envelope, whose values
  are all strings, so the JCS **number**-serialization algorithm — which RFC 8785 §3.2.2.3 declines to
  specify — is neither implemented nor reachable. A general-purpose JCS function for third-party
  documents is therefore absent, which is the honest boundary. The digest is a typed `ActionDigest` on
  the wire and in the durable record, so a malformed one is refused at the trust boundary as
  `request.invalid` rather than compared as text and reported as a mismatch. Cross-language vectors are
  still outstanding (below).
- **The envelope covers the contract's own input list except the two entries with no producer.**
  `fingerprint_version`, principal, workspace, the tool capability, source identity, and schema
  fingerprint, the normalized arguments, and — **separately from the identity** — the `effects` and
  `risk`. The last two are not redundant: `ToolIdentity::schema_fingerprint` covers the **input schema
  only**, so a tool reclassified from reading to deleting keeps its identity, and a fingerprint built
  from the identity alone would keep its fingerprint too — letting an approval granted for a read
  authorize a delete. Effects are canonicalized as a **set** (sorted, deduplicated) so one effect set
  has one fingerprint. Still absent, because nothing produces them: "material content/artifact hashes"
  and "target connector account/resources".
- **Approval creation.** `ApprovalRepository::request` is the only writer and no caller exists: a
  request comes from a policy `Ask` decision on a tool call, and there is no executor, so
  `tests/e2e/approval-journey.mjs` seeds the row directly. The *decision* path is live; the
  *creation* path is not.
- **List filters and cursors.** `limit` is served and the response carries **`has_more`**, so a client
  can tell a full page from a complete one; the channel predicate runs **inside the query** (see below).
  **`risk` is served too** (`?risk=critical`), and it is the first filter beyond `limit`/`cursor` to be:
  it narrows the same `WHERE` the `LIMIT` bounds, so a client asking for the prompts that will demand a
  step-up gets a full page of them rather than a page of everything with the rest discarded. It was the
  filter whose meaning changed when the step-up rule became real — a `critical` prompt is the one an
  ordinary session cannot decide — which is why it is the one added first. An unrecognised *value*
  (`?risk=severe`) is refused like an unknown *key*, because treating it as "no narrow" would return a
  **superset** of what the caller asked for. The remaining filters — `state`, `effect`, requesting
  run/tool, and the time filters — are refused **by name** rather than silently ignored, each still
  needing its own indexed query, for the same superset reason.

  **A cursor is now served too** (`?cursor=`, answered as `next_cursor`), because `has_more` without one
  was a dead end: the daemon told a client that more prompts awaited a decision and gave it no way to
  fetch one — the worst of both, since the client knows work remains and cannot do it. The earlier note
  here said a cursor "would claim a stable position the listing does not yet guarantee across a
  concurrent decision", and **that was wrong about the position**: the order is `(expires_at, id)`,
  `expires_at` is an immutable column and `id` is primary, so the order is total and stable. What a
  concurrent decision changes is *membership*, not order — which is exactly why the cursor is a
  **keyset** position (`(expires_at, id) > (last.expires_at, last.id)`) rather than an offset. An
  offset *would* be the unstable choice: a row decided between two fetches shifts every later row by
  one, so page two would skip an approval, and a skipped approval is a prompt nobody decides. The value
  is opaque (a versioned base64 payload) so a client passes it back rather than constructing one, it
  carries the channel it was minted for, and replaying it against another channel is
  `request.invalid_cursor` rather than an empty page — because the empty page *is* the disclosure.
- **Consumption (`CONSUMED`) and invalidation (`INVALIDATED`).** Both are reached by the exact
  tool-call reservation (`TLS-006`'s ledger) at invocation time; the ledger exists and nothing reserves
  through it, so those two edges have no caller.
- **The durable expiry worker.** The contract's Expiry rule names two evaluators — "every
  read/decision/reservation" **and** "a durable expiry worker" — and only the first exists. Reads now
  record every lapse they see, so a row is expired the first time anybody looks at it; but a row
  **nobody looks at** stays `pending` in storage indefinitely, and its own `expires_at` column is the
  only record of a deadline that has passed. That is a real gap rather than a stylistic one: a
  workflow waiting on that approval has nothing to wake it, and the store accumulates records whose
  state contradicts their deadline. The worker is the same shape as the run reaper and belongs with the
  scheduler, so it is deferred to that slice rather than guessed at here.
- **The outbox/resume signal.** Decide step 6 writes "the immutable decision and outbox/resume signal
  atomically". The transition and its audit row are written in one transaction here; the event is
  `AUT-004`'s outbox, and a waiting run's resume belongs to that slice.
- **Redaction of the preview.** The preview is structured and bounded, and its values are stored
  verbatim as the record of what the user was shown — but the *producer* redacts and no producer
  exists. `TLS-005` recorded this; the shape is unchanged.
- **Channel assurance is now required, and it is derived rather than configured.** The record still has
  no field naming a *required* level, and that was the reason this bullet used to say a step-up approval
  was "unrepresentable rather than merely unenforced". The rule is instead **derived from the record's own
  risk**: `RequiredAssurance::required_for(risk)` makes a `Critical` action require `Elevated`, which is
  this contract's "critical actions default to step-up in CLI/desktop/mobile" stated once. `decide`
  enforces it after the channel check, so `approval.assurance_insufficient` — which was in the stable-error
  list with **no producer** — is now reachable, `403`, and non-retryable, and an ordinary session can no
  longer answer the one class of prompt the risk label exists to make a user step up for. The threshold is
  a floor rather than a ceiling: a policy may require more, and `High` deliberately does not step up,
  because stepping up everything above `Moderate` would make the label meaningless. **What is still
  missing is a per-record override** — an approval may not require *more* than its risk implies, because a
  field for that would need a producer (policy) that does not exist.
- **The detail view's remaining named inputs.** "Related run" is served as `run_id`, and the schema
  fingerprint now is too, but the detail requirement's full list is not complete: there is no
  `output_schema` fingerprint, no artifact or content hash (nothing produces one), and no resolution of
  a connector account or resource (no connector adapter exists). Each is an added field when its
  producer arrives rather than a missing rule.
- **Generated OpenAPI and cross-language fingerprint vectors.** Neither exists; this contract's test
  list names both. The **Rust** half of the vector requirement is now covered — the canonicalizer is
  asserted against RFC 8785's own §3.2.3 sample order, including a key outside the basic multilingual
  plane where a byte-wise sort and the specified UTF-16 sort disagree — but a second language's
  implementation is the remaining increment.

This section and `approval_service`'s module doc are where the absences live, deliberately: a reader of
either should not have to reconstruct which half is real.
