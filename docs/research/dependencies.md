# Routine Dependency Evidence

Status: ACCEPTED
Last reviewed: 2026-09-21

This ledger is the shortened evidence path allowed by the
[integration research policy](integration-research-policy.md). It is only for a
library that meets every criterion below.

## Eligibility

A dependency is routine only when it:

- does not call or model an external service, API, provider, or protocol;
- does not define a public wire type or persisted format;
- does not own authentication, authorization, cryptography, secrets, sandboxing,
  code execution, network policy, or update verification;
- does not own database, migration, queue, process, filesystem, ACL, keychain,
  service-manager, installer, or cross-platform semantics;
- does not determine a supported target or release artifact;
- can be replaced without changing a JARVIS contract or operator workflow.

If any criterion is false or uncertain, create a full integration evidence note.
Rust, Tokio, Axum/Tower, SQLx, SQLite, credential stores, service managers,
installers/updaters, telemetry exporters, cryptography, and provider SDKs are not
routine for their boundary-affecting use in JARVIS.

## Required Row

Before adding an eligible package, record one row with an exact version. Verify
the official package/repository, release notes, license file, maintenance state,
minimum supported Rust/runtime version, enabled features, transitive risk, and
the reason the standard library or an existing dependency is insufficient.

| Package | Exact version | Official source and release notes | License evidence | Purpose and enabled features | Replacement boundary | Reviewed | Revalidate | TODO |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| None eligible to date | N/A | N/A | N/A | Every Foundation dependency failed an eligibility criterion below | N/A | 2026-09-21 | At the next dependency change | `FND-000` |

## Review Rules

- One row never approves a version range or similarly named package.
- Feature changes and major/minor upgrades require re-review.
- A vulnerability, abandonment signal, license change, or MSRV change marks the
  row `STALE` until reviewed.
- Dependency source code may clarify behavior, but copied code remains subject
  to the project license and notice policy.
- `cargo deny`, vulnerability scanning, and SBOM output complement this review;
  they do not replace it.
