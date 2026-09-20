# Integration Evidence: <Name>

Status: PROPOSED
Lifecycle: TEMPLATE
Owner: <team or person>
Last verified: YYYY-MM-DD
Revalidate by: YYYY-MM-DD
Implementation gate: NOT READY

## Decision Summary

- Purpose:
- JARVIS boundary:
- Proposed package or protocol version:
- Supported deployment modes:
- Explicitly unsupported:
- Kill switch or disable path:

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` |  |  |  |  |
| Specification |  |  |  |  |
| API schema |  |  |  |  |
| Authentication |  |  |  |  |
| Security |  |  |  |  |
| Limits/pricing |  |  |  |  |
| Changelog |  |  |  |  |
| SDK source |  |  |  |  |
| SDK examples/tests |  |  |  |  |

Attempted `llms.txt` URLs that did not exist:

- None recorded.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| API/spec |  |  | UNVERIFIED |
| SDK |  |  | UNVERIFIED |
| Wire protocol |  |  | UNVERIFIED |

## Contract

### Authentication and Authorization

- Credential type:
- Credential placement:
- Required scopes:
- Refresh/rotation behavior:
- Tenant or workspace binding:
- Webhook signature verification:

### Transport and Lifecycle

- Endpoint(s):
- Transport(s):
- Connection lifecycle:
- Negotiation/versioning:
- Streaming frame shape and terminal event:
- Ordering and duplication:
- Cancellation and timeout:
- Reconnect/resume:

### Data and Limits

- Request/response schemas:
- Pagination:
- Payload and concurrency limits:
- Rate-limit headers and behavior:
- Retention and privacy:
- Data residency:
- Cost assumptions:

### Errors and Retries

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Invalid credentials |  | No |  |
| Insufficient scope |  | No |  |
| Rate limited |  | Conditional |  |
| Timeout |  | Conditional |  |
| Provider unavailable |  | Yes |  |
| Invalid request |  | No |  |

## Security Analysis

- Trust boundaries:
- Prompt-injection exposure:
- Secret leakage paths:
- SSRF or callback risks:
- Tool side effects:
- Required approvals:
- Redaction rules:
- Sandbox or network policy:
- Abuse cases:

## Normalization Map

| Provider concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
|  |  |  |

Do not expose provider SDK types in JARVIS domain contracts.

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| C-001 |  | UNVERIFIED |  |  |

## Test Plan

### Deterministic Tests

- [ ] Schema and normalization
- [ ] Policy and scope enforcement
- [ ] Error mapping
- [ ] Retry/idempotency behavior
- [ ] Redaction

### Contract Fixtures

- [ ] Real, safely redacted payload captured
- [ ] Fixture provenance and capture date recorded
- [ ] Official schema validation included
- [ ] Malformed and forward-compatible payloads covered

### Gated Live Tests

- [ ] Authentication and capability discovery
- [ ] One representative read
- [ ] One representative write in a dedicated test account, if applicable
- [ ] Cancellation/timeout
- [ ] Rate-limit or simulated backoff
- [ ] Cleanup leaves no external resources

## Operational Readiness

- [ ] Health probe
- [ ] Safe diagnostics
- [ ] Metrics and trace fields
- [ ] Setup and reauthentication
- [ ] Disable/unload
- [ ] Migration
- [ ] Credential rotation
- [ ] Provider outage behavior
- [ ] Operator runbook

## Open Questions

- None recorded.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| YYYY-MM-DD | Initial research |  |