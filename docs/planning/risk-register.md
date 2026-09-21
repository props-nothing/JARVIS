# Risk Register

Status: ACCEPTED
Review state: ACTIVE
Last reviewed: 2026-09-20

Likelihood/impact use `Low`, `Medium`, `High`, `Critical`. Owners are roles until
maintainers are assigned.

| ID | Risk | Likelihood | Impact | Mitigation and trigger | Owner |
| --- | --- | --- | --- | --- | --- |
| `R-001` | Scope expands faster than vertical proof | High | High | Milestone gates; first slice excludes integrations/UI; no parallel placeholder features | Product/architecture |
| `R-002` | Framework/provider becomes accidental core | Medium | Critical | Ports/adapters, domain dependency checks, ADR-0001/0005/0006 | Architecture |
| `R-003` | Fast-changing API assumptions become stale | High | High | Evidence gate, expiry dates, captured fixtures, gated live tests | Integration owner |
| `R-004` | Cross-platform service/paths/install behavior diverges | High | High | Native CI and clean-machine packaged journeys from Milestone 1 | Release |
| `R-005` | Loopback API is mistaken for trusted local access | Medium | Critical | Enrollment token, ACL, Host/Origin tests, no privileged cookies | Security |
| `R-006` | Prompt injection drives data exfiltration/tool effects | High | Critical | Minimal context/tools, deterministic policy, exact approval, abuse tests | Security/tools |
| `R-007` | Crash/retry duplicates communication or physical effect | Medium | Critical | Idempotency ledger, ambiguous reconciliation, crash failpoints | Workflow/tools |
| `R-008` | SQLite/PostgreSQL semantics drift | Medium | High | Shared repository contract suite, semantic migration IDs | Storage |
| `R-009` | Memory stores false/sensitive/cross-workspace claims | High | Critical | Explicit-memory-first, provenance, confirmation, query-time scope, correction | Memory/security |
| `R-010` | Runtime/plugin supply chain expands trusted base | High | Critical | Process isolation, provenance/signatures, no auto-grant, quarantine | Runtime/security |
| `R-011` | Sandbox parity is weaker on one OS | High | Critical | Per-OS threat/tests, feature availability labels, deny unsupported profiles | Execution/security |
| `R-012` | Voice caller identity is spoofed | High | Critical | Guest baseline, signed session token, step-up, no caller-ID trust | Voice/security |
| `R-013` | Outbound calling violates consent/law or duplicates calls | Medium | Critical | Region policy, explicit consent/approval, quiet hours, idempotency, test-only first | Voice/legal |
| `R-014` | Provider cost/latency causes denial of wallet or poor voice | High | High | Hard budgets, routing telemetry, concurrency limits, kill switches | Models/operations |
| `R-015` | Observability leaks private content/secrets | Medium | Critical | Allowlisted fields, canaries, local default, support preview | Observability/security |
| `R-016` | Migration/update strands or corrupts local users | Medium | Critical | Compatibility guard, verified backup, native upgrade/rollback tests | Storage/release |
| `R-017` | Too many crates/services slow development | Medium | Medium | Bootstrap crate minimum; evidence/ADR before split or infrastructure | Architecture |
| `R-018` | Native workflow engine grows into a poor Temporal clone | Medium | High | Constrained feature set, measured adoption trigger, adapter boundary | Workflow |
| `R-019` | License choice/dependency licensing blocks release | High | High | Select project license before code distribution; automated policy/SBOM | Project owner |
| `R-020` | No private security contact or release key process | High | Critical | Must exist before external release; protected environment and rotation drill | Project owner/security |
| `R-021` | Tests pass only against self-authored fixtures | High | High | Real captured fixtures and gated live provider tests | Quality/integrations |
| `R-022` | Documentation drifts from code/contracts | Medium | High | Generated schemas, link/traceability checks, docs in definition of done | All owners |
| `R-023` | A voice-carrier attribute default changes and silently alters turn-taking or barge-in | Medium | High | Set every semantic attribute explicitly, assert the serialized request, re-read the reference page at each evidence refresh | Voice/integrations |
| `R-024` | A voice latency claim is carried across pipelines and becomes an architectural assumption nobody measured | High | High | Attribute every measurement to a pipeline, report components separately, re-measure per pipeline in `ACC-068` | Voice/quality |
| `R-025` | Promoted voice research is mistaken for measured evidence, so a latency or capability claim is cited from a note after the raw capture it came from is deleted | Medium | High | Promoted notes label each claim with its provenance and re-measure in `ACC-068`; keep only runnable harnesses out of the repository and re-capture before citing an `OBSERVED` value | Research/architecture |
| `R-026` | A voice-agent framework's tools, MCP wiring, or state become a second control plane | Medium | Critical | Runtime adapter with scoped grants, canonical tool validation, ADR-0005/0007 review before adoption | Runtime/architecture |
| `R-027` | Model "streaming" is treated as a boolean, so a streamed voice pipeline silently degrades to full-generation latency | Medium | High | Measure time-to-first-token **and** token spread per model; treat an unmeasured streaming claim as `UNVERIFIED` | Models/voice |
| `R-028` | A transport's own permission model, participant attribute, or room metadata is mistaken for JARVIS authorization, letting a peer widen its own scope | Medium | Critical | Track-granular grants re-checked at publication; transport permissions and attributes recorded as observations only; `ACC-080` | Media/security |
| `R-029` | A realtime transport's peer-to-peer method call becomes an unmediated tool path, giving an untrusted participant a function-call primitive with no policy | Medium | Critical | Fixed JARVIS method set, schema-validated arguments, canonical policy path, bounded payload and timeout, `ACC-081` | Media/tools |
| `R-030` | A realtime transport is adopted because its agent features are attractive, so a vendor control plane enters through the transport decision | Medium | Critical | Two-axis rule in ADR-0011; transport and framework decided separately under `RTM-009`/`RTM-010`; canonical session state only | Architecture/media |

## Escalation Rules

- Any `Critical` impact risk blocks the feature/profile unless its controls and
  evidence are complete or the feature is disabled by default.
- Increasing risk likelihood/impact requires an ADR or milestone replan when it
  affects architecture or release scope.
- A production incident creates or updates a threat, test, runbook, and evidence
  note; a code-only fix is incomplete.

## Infrastructure Adoption Triggers

Do not add a distributed dependency merely to reduce a risk in theory. Record
measurements first.

| Candidate | Evidence that can trigger evaluation |
| --- | --- |
| Redis | Measured database/cache contention or ephemeral coordination latency |
| NATS/Kafka | Durable event throughput/fan-out/retention exceeds database design |
| Temporal | Workflow volume/duration/versioning/worker topology exceeds native engine |
| Qdrant/vector DB | pgvector/local recall, latency, scale, or operations fail SLO |
| Kubernetes | Deployment count/availability/operations justify orchestration cost |

Adoption still requires an ADR, failure/backup model, security review, and local
mode must remain unaffected unless the product explicitly changes.