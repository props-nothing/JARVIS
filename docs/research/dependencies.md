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
| `futures-core` | `0.3.34` | crates.io `futures-core` 0.3.34; source `rust-lang/futures-rs` (`futures-core` crate); release notes in the repository's `CHANGELOG.md` | `MIT OR Apache-2.0` (dual, standard Rust ecosystem terms); `LICENSE-APACHE` and `LICENSE-MIT` in the crate source | Supplies the `Stream` trait so the local control API's event stream can be a streaming response body, which `docs/contracts/local-control-api.md` requires ("Initial connection replays retained events from sequence 1, then follows live events"). Enabled features: **none** (`default-features = false`). The standard library has no async stream trait and the workspace does not depend on `futures-util`, so there is no smaller substitute | The trait is used only to make one response body a stream. It is replaceable by `futures-util` (a superset, already transitively present) or by `tokio-stream`, whose `Stream` is a re-export of this crate's — so replacing it changes no JARVIS contract and no operator workflow. **No new crate and no version change**: `futures-core` 0.3.34 was already resolved (a leaf of `axum`'s and `sqlx`'s trees), so the `Cargo.lock` diff adds one line inside `jarvis-infrastructure`'s dependency list | 2026-09-27 | At the next dependency change | `BRN-007` |
| None further eligible to date | N/A | N/A | N/A | Every other Foundation dependency failed an eligibility criterion below | N/A | 2026-09-21 | At the next dependency change | `FND-000` |

## Review Rules

- One row never approves a version range or similarly named package.
- Feature changes and major/minor upgrades require re-review.
- A vulnerability, abandonment signal, license change, or MSRV change marks the
  row `STALE` until reviewed.
- Dependency source code may clarify behavior, but copied code remains subject
  to the project license and notice policy.
- `cargo deny`, vulnerability scanning, and SBOM output complement this review;
  they do not replace it.
