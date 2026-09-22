# Model Data Policy Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT
Owner: Model Gateway and Privacy

## Purpose

This contract controls where model inputs may go and which documented provider
data practices are acceptable for a call. It separates user/workspace policy
from provider capability evidence. A model name, pricing tier, marketing term,
or unchecked UI toggle is never proof of retention, training, residency, or
telemetry behavior.

## Policy Record

```json
{
  "policy_id": "019...",
  "workspace_id": "019...",
  "version": 4,
  "name": "Private client work",
  "status": "active",
  "rules": {
    "locality": "local_only",
    "allowed_providers": [],
    "allowed_models": [],
    "maximum_provider_retention": "none_documented",
    "provider_training_use": "disallowed_documented",
    "telemetry": "disabled",
    "allowed_residency_regions": ["eu"],
    "maximum_sensitivity": "confidential",
    "allow_fallback": false
  },
  "created_at": "2026-09-20T12:00:00Z",
  "updated_at": "2026-09-20T12:10:00Z"
}
```

Initial rule values are typed enums, not free-form provider strings.

### Locality

```text
local_only
private_network_allowed
approved_cloud_allowed
```

### Provider Retention Requirement

```text
none_documented
bounded_documented
provider_default_allowed
```

`none_documented` means current official evidence states the selected account,
endpoint, feature, and request mode meet the required retention behavior. It is
not a universal promise that JARVIS can independently prove deletion.

### Training Use Requirement

```text
disallowed_documented
account_policy_allowed
provider_default_allowed
```

The gateway exposes provider-owned guarantees as `DOCUMENTED` or `OBSERVED`, not
as JARVIS guarantees. An `UNVERIFIED` claim cannot satisfy a hard rule.

## Precedence

Policy is merged in this order, where an earlier deny/hard constraint cannot be
weakened by a later layer:

1. legal, deployment, and administrator deny rules;
2. workspace policy and data-classification ceiling;
3. resource/document sensitivity policy;
4. task/run explicit restrictions;
5. user preferences;
6. provider/model defaults.

Explicit model/provider selection is still subject to all higher hard rules.
Policy exceptions are separate durable records; editing a request body cannot
create one.

## Provider Capability Evidence

Every routable provider/model capability snapshot records:

```text
integration evidence ID and note revision/hash
API/SDK/account/product tier and feature version
capability key and value
evidence label: VERIFIED, DOCUMENTED, OBSERVED, INFERRED, UNVERIFIED, or STALE
official source URL and access/expiry dates
test-account observation or contract fixture reference where applicable
```

Expired, `STALE`, `INFERRED`, or `UNVERIFIED` evidence cannot satisfy a hard
retention/training/residency rule. A provider setting that requires account-side
configuration is unavailable until health/diagnostics confirms that setting for
the credential/account being routed.

## Requested and Effective Decision

Each model call resolves and persists:

```json
{
  "policy": {"policy_id":"019...","version":4},
  "requested": {
    "locality":"local_only",
    "maximum_provider_retention":"none_documented",
    "provider_training_use":"disallowed_documented",
    "telemetry":"disabled",
    "allowed_residency_regions":["eu"],
    "sensitivity":"confidential"
  },
  "effective": {
    "provider_id":"local.ollama",
    "model_id":"example-model",
    "endpoint_class":"local",
    "retention":"not_applicable_local",
    "training_use":"not_applicable_local",
    "telemetry":"disabled",
    "residency":"local_device"
  },
  "evidence_refs": [],
  "rejected_candidates": []
}
```

The effective object states what the gateway selected based on current evidence;
it does not claim hidden provider behavior. Failure to find a compliant route is
`model.policy_unsatisfied`, not permission to silently relax policy.

## Exceptions

An exception binds:

```text
workspace and granting principal/assurance
base policy ID/version
exact relaxed rule and maximum scope
provider/model/task/resource selectors
reason and user-visible consequence
issued, expiry, revocation, and single-use state
```

Exceptions cannot override non-waivable legal/administrator denies. Sensitive or
cross-border exceptions require policy-defined step-up/approval. They expire by
default and are included in the route decision/audit without exposing content.

## Persistence

`model_data_policies` stores immutable versions; changing rules creates a new
version. `model_policy_exceptions` stores separately revocable relaxations.
`model_route_decisions` stores the resolved policy/version, requested and
effective data policy, evidence references, candidate rejection reason codes,
and exception reference. Model calls reference the route decision they used.

Historical records retain the policy/evidence version needed to explain a past
decision even after current evidence expires. Replaying a call re-evaluates
current policy; it does not inherit an old permission silently.

## API

Authenticated product clients use:

```text
GET /api/v1/model-data-policy
PUT /api/v1/model-data-policy
GET /api/v1/model-data-policy/effective
```

`GET` returns the active workspace policy, version, source layers, user-visible
provider evidence freshness, and any policy-controlled fields the caller may
change. It never returns credentials or private provider account responses.

`PUT` requires `Idempotency-Key`, `expected_version`, and only fields authorized
for the principal. Unknown fields and unsupported enum values fail. The server
persists a new version and returns it; concurrent modification returns
`resource.version_conflict`.

`effective` evaluates a bounded, schema-defined classification/capability query
without accepting arbitrary prompt content and returns compliant candidates or
safe rejection reason codes. It is diagnostic, not a way to force a route.

Create-run requests reference a policy ID/version or request stricter typed
overrides. They cannot submit provider capability claims or relax resolved
workspace policy.

## Errors and Degraded Behavior

Minimum stable codes:

```text
model.policy_not_found
model.policy_version_conflict
model.policy_unsatisfied
model.evidence_missing
model.evidence_stale
model.exception_required
model.exception_expired
```

If evidence expires while a run is waiting, re-evaluate before the next model
call. Hard policy fails closed. A soft preference can use a compliant fallback
only when the recorded policy permits it and the user-visible activity names the
degradation.

## Required Tests

1. precedence and non-weakening across every policy layer;
2. requested/effective serialization and immutable version history;
3. stale, contradictory, account-tier-specific, and `UNVERIFIED` evidence;
4. local-only, provider allow/deny, retention, training, telemetry, residency,
   sensitivity, fallback, and explicit pin interactions;
5. exception scope, step-up, expiry, revocation, and replay;
6. concurrent update, idempotency conflict, unknown fields, and safe errors;
7. routing rejection rather than silent relaxation;
8. redaction of provider account details and user content;
9. generated schema/API drift and user-visible requested/effective explanation.

These tests provide evidence for `ACC-013`, `ACC-017`, and `ACC-018`.

### Implemented evidence (Milestone 2)

`jarvis_domain::model::policy` implements the rule and precedence half of this
contract; `jarvis_domain::model::capability` implements the evidence half. Test 1
(precedence and non-weakening) and test 3 (stale and `UNVERIFIED` evidence) have
executable evidence:

| Rule here | Enforced by | Falsified by |
| --- | --- | --- |
| an earlier deny cannot be weakened by a later layer | `PolicyRules::merge_stricter` only narrows, and an absent layer merges against a permissive identity | `an_absent_layer_cannot_loosen_a_deny`; `a_stricter_later_layer_still_narrows_an_earlier_one` is the case a "first deny wins" shortcut gets wrong |
| the precedence order is the contract's order | `PolicyLayer` declaration order with `ResolvedPolicy::merge` sorting by it | `the_layer_order_matches_the_contracts_precedence_list`, plus layers supplied out of order |
| allow/deny narrows to the intersection | `PolicyRules::merge_stricter` | `an_allow_list_narrows_to_the_intersection` |
| a contradictory policy is reported, not permitted | two disjoint allow-lists are `jarvis.invalid_policy_layer` | `two_disjoint_allow_lists_are_refused_rather_than_resolved_empty` |
| an unrestricted layer is not "nothing allowed" | an empty set means "no restriction at this layer" | `an_unrestricted_layer_does_not_erase_a_restricted_one` |
| expired and `UNVERIFIED` evidence fail a hard rule | `Evidence::satisfies_hard_requirement_on` is one predicate over label **and** date | `evidence_is_valid_through_its_revalidation_day_and_stale_after_it`, `only_verified_evidence_satisfies_a_hard_requirement` |
| provider guarantees are not JARVIS guarantees | `EffectiveDataPolicy` has no such field to populate | `the_effective_policy_has_no_provider_guarantee_field` asserts the shape |

The last row is the contract's own distinction made structural rather than
documented: `model.evidence_missing` and `model.evidence_stale` are reported by
`RejectionReason::EvidenceMissing` and `RejectionReason::EvidenceStale`, and an
exception is visible on the decision through `ModelRouteDecision::relied_on_exception`
rather than inferred from whether the route looks permissive. All seven `model.*`
codes this contract fixes are exposed verbatim by `jarvis-domain::error`, and a
test asserts each string so a rename cannot silently change a client-visible code.

`EffectiveDataPolicy::is_self_consistent` refuses the one inconsistency the shape
does not prevent on its own: a **cloud** endpoint recorded as
`not_applicable_local` for retention or training use, or a **local** endpoint
recorded with a provider retention classification. Both read as *more* careful than
the route is, and neither is visible without a check because the two fields are set
independently.

#### Test 7: routing rejection rather than silent relaxation

`jarvis_domain::model::routing` is the **selector**: `select_route` takes a bounded
candidate list and a `RouteRequest` and returns the first compliant candidate or
`model.policy_unsatisfied`. Before this the contract had the vocabulary and none of the
behaviour — `RejectionReason` was defined with nothing to produce it and
`ModelRouteDecision` with nothing to construct it, so a "rejected candidate" could not
exist. Three rules are structural:

- **There is no path that relaxes a hard rule.** A candidate that cannot satisfy the
  resolved policy or attest a required capability is rejected *with a reason*; nothing
  downgrades a requirement because no candidate met it. That is the difference between a
  filter and a preference, and it is why selection is not scored: a score could rank an
  incompliant candidate above a compliant one.
- **Every rejection is recorded, not only the winner.** A decision that named just its
  selection could not answer "why not the local model", which is the question an operator
  asks. The list is bounded (`MAX_CANDIDATES`) because it comes from a provider.
- **Retention and training use are candidate-attested, never inferred.** Whether a
  provider documents bounded retention is a fact about its current published terms, so
  deriving `none_documented` from "it is a cloud endpoint" would claim a documented finding
  from a category. A candidate with no usable note is classified `provider_default` — a new
  `EffectiveRetention` variant meaning "the provider's own default terms were accepted",
  which **documents nothing** — and a policy demanding a documented statement rejects it.
  An earlier version of this module classified an undocumented cloud candidate as
  `none_documented`, which is precisely the promotion this contract forbids: it would make
  the least documented route read as the most careful kind. A test caught it.

Two evidence predicates now exist, and using one for both would fail in one direction
whichever was chosen. `Evidence::satisfies_hard_requirement_on` admits **only** `VERIFIED`,
because a capability can differ between a provider's documented version and the one this
repository pins. `Evidence::satisfies_data_policy_rule_on` admits `VERIFIED`, `DOCUMENTED`,
and `OBSERVED`, because this contract names the labels that cannot satisfy a
retention/training/residency rule ("expired, `STALE`, `INFERRED`, or `UNVERIFIED`") and the
official documentation is exactly the source that establishes a retention term.

`RouteCandidate::region` is an `Option`, and a missing region **fails** an allow-list rather
than passing it: a provider that does not publish where it processes cannot be shown to be
inside an allowed region, and refusing is recoverable while sending is not.

#### The API surface: how the selector is reached

`jarvis_infrastructure::http::policy` implements the two `GET`s. Each resolves the authenticated
scope, delegates to `PolicyService`, and renders the result; the spellings live in spelling
functions rather than the domain's `Display`, because the two vocabularies differ:

| Rule here | Enforced by | Falsified by |
| --- | --- | --- |
| the wire vocabulary is the contract's, not the domain's | one spelling function per value family in `http::policy` | `every_rule_value_is_rendered_with_the_contracts_spelling`, which enumerates every variant |
| a rejection reason is a **code**, not a sentence | `rejection_reason_of` rather than `RejectionReason::to_string()` | `every_rejection_reason_is_a_code_rather_than_a_sentence` |
| `effective` is diagnostic, not a way to force a route | the handler takes no model argument, and the probe sensitivity is a constant | the route tests, plus the absence of a parameter to pass one |
| a refusal is `200` with a body, not an error status | `PolicyServiceError::Unsatisfied` renders as `200` | `the_effective_route_endpoint_refuses_a_candidate_the_policy_excludes` |
| nothing names a provider account or credential | the response types carry only rule values and a provider-qualified model | the response shape itself has no field for one |
| another workspace's policy reads as absent | the scope is resolved server-side and passed to every store call | `the_policy_surface_hides_another_workspaces_policy`, which reads the row back by its own workspace to prove it exists |

The rejection-reason row is a defect that was found rather than designed. `rejected_view`
rendered `RejectionReason`'s `Display`, which is human prose for an operator log, so the wire
carried `"locality violated"` where the contract's own response example shows
`locality_violated`. Every other value in the module already went through a spelling function;
this one had none, and no test asserted the string — the one rejection code a handler test
reached was produced by the domain, so the test agreed with the defect. The spelling test now
enumerates all nine variants and asserts each is a snake_case code that differs from the domain's
`Display`, which makes the *class* fail rather than the instance.

**Now done:** `jarvis_application::policy_service::PolicyService` reads the **stored** policy and
constructs the `RouteRequest` from it, so `select_route_explained` has a caller in production code.
`GET /api/v1/model-data-policy` returns the active version; `GET /api/v1/model-data-policy/effective`
probes the configured candidates against it. Both are described under *API* above.

**Still not done.** Three things, each named rather than implied:

- `PUT /api/v1/model-data-policy` has no handler, so a policy can only be written through the
  repository port. The `expected_version` precondition and the `resource.version_conflict`
  response this contract requires are implemented in the store — `insert_version` refuses a
  version that already exists — but there is no route that reaches it.
- `model_policy_exceptions` has a table and no code: no exception can be granted, expired,
  revoked, or attached to a decision, so `ModelRouteDecision::exception_ref` is always absent and
  the contract's *Exceptions* section is unimplemented. Nothing enforces a hard rule through an
  exception, which is why no route can relax one.
- `CreateRunRequest.model_policy` is parsed by the API and **not read**, so a create-run request's
  stated policy ID/version does not reach `RunService::create`. A run therefore evaluates under
  the workspace's active policy and a client's explicit reference is silently ignored — the
  request is accepted and the field has no effect. This is recorded as a defect in `TODO.md`
  rather than left as a silent gap, because "accepted and ignored" is indistinguishable to a
  client from "accepted and applied".

`ProviderInventory` reports candidates whose `region`, `retention`, and `training_use` are all
`None`, which is the honest state until `BRN-011` measures capabilities and an adapter attaches
provider terms. The consequence is deliberate: a policy demanding **documented** retention or
training use refuses every candidate rather than having a claim inferred for it, so an operator
who writes a strict policy sees `model.policy_unsatisfied` with `retention_unsatisfied` reasons
rather than a route that was permitted on an assumption.

#### The persistence half: the policy store

`migrations/sqlite/000004_model_data_policy.sql` creates the three tables this contract's
`Persistence` section names — `model_data_policies`, `model_policy_exceptions`, and
`model_route_decisions` — raising the schema to version 4 with the minimum reader left at 1,
because every existing table and column is untouched.
`jarvis_application::repository::policy` is the port and
`jarvis_infrastructure::storage::repositories::policy` the SQLite adapter, with an in-memory
double in `jarvis_application::testing` so a service test can register a policy without a
database.

The immutability rule is now structural rather than documented. `insert_version` is an
`INSERT` behind `UNIQUE (policy_id, version)`, and a duplicate is reported as
`storage.version_conflict` — the same typed outcome the run state machine uses, because it is
the same fact: someone advanced the record since this caller read it. Two decisions follow
from the contract:

- **The caller states the version, and the store checks it.** Assigning the next version
  inside the store would make a concurrent update indistinguishable from a sequential one,
  and this contract requires `resource.version_conflict` for the former. A store that chose
  the number could only ever report success.
- **Rules are stored as JSON, not as twenty columns.** `PolicyRules` has seven enums and four
  sets, so a flattened schema would be twenty places for the stored form and the domain type
  to disagree with no single reader able to notice. A row whose rules cannot be reinterpreted
  is reported as `Corrupted` rather than as absence, because reporting it absent would let a
  caller create a replacement for a policy that is still there.

`load_active` requires an ordering rule, since a workspace may legally hold two active
versions (reactivating an older one without archiving the newer). It takes the highest
version, and the in-memory double implements the same rule — a difference between the two
would be invisible to a test that exercised either one alone. An absent active policy is
`NotFound` rather than an empty one: a call made under no policy applies nothing, so the
caller has to decide whether that is permitted, and it cannot decide if the store hands back
`PolicyRules::permissive()`.

**Still not done:** `select_route` remains uncalled, because nothing yet assembles a
`RouteRequest` from a loaded policy plus a candidate inventory. The store is the input; the
wiring and the `GET /api/v1/model-data-policy/effective` surface are the next increment. The
API endpoints, the exception lifecycle (issue/expire/revoke/single-use), and the
`resource.version_conflict` HTTP mapping are all absent.



**Not done**: tests 2, 4 through 9 have no executable evidence yet. There is no
persistence for `model_data_policies`, `model_policy_exceptions`, or
`model_route_decisions` (that is `BRN-004`), so immutable version history,
concurrent update, idempotency conflict, exception expiry/revocation/replay, and
generated-schema drift are unimplemented rather than verified.