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

**Not done**: tests 2, 4 through 9 have no executable evidence yet. There is no
persistence for `model_data_policies`, `model_policy_exceptions`, or
`model_route_decisions` (that is `BRN-004`), so immutable version history,
concurrent update, idempotency conflict, exception expiry/revocation/replay, and
generated-schema drift are unimplemented rather than verified.