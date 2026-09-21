# Integration Evidence: Release Signing and Verification

Status: ACCEPTED
Lifecycle: ACTIVE
Owner: Foundation
Last verified: 2026-09-21
Revalidate by: 2027-03-20
Implementation gate: PASSED

## Decision Summary

- Purpose: prove that a release manifest and the artifacts it lists are authentic
  before any byte is executed or installed, and that a modified byte or a
  rewritten manifest is refused on the consumer's machine.
- JARVIS boundary: `jarvis_infrastructure::release`. It is a pure verification
  library plus one CLI command (`jarvis verify-release`). It reads a manifest, a
  detached signature, and local artifact files. It performs no network access, no
  installation, and no publication, and it never writes to the profile.
- Proposed package or protocol version: `ed25519-dalek` **`=3.0.0`** for the
  signature primitive, and a detached-signature manifest format defined by JARVIS
  (`RELEASE_MANIFEST_SCHEMA_VERSION = 1`). No release framework, no transparency
  log, and no signing service is adopted at this stage.
- Supported deployment modes: local verification beside a downloaded release, and
  CI verification of a test-signed release on every tier-1 native lane.
- Explicitly unsupported: production signing identities and custody, key
  rotation, revocation publication, TUF-style metadata rollback defense,
  reproducible-build attestation, and any supply-chain service (Sigstore, cosign,
  in-toto). Those are `OWN-003`, `OWN-004`, and later release work.
- Kill switch or disable path: the trust store is compiled in, so revoking a key
  is a code change that ships a new binary; deleting `scripts/release-verify-smoke.mjs`
  and its workflow step removes the CI check without affecting the product. No
  runtime dependency on any signing service exists, so there is nothing external
  to disable.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | none published | N/A | 2026-09-21 | Recorded honestly rather than omitted — see the attempts below |
| Crate metadata | `https://crates.io/api/v1/crates/ed25519-dalek` | `3.0.0`, published 2026-07-06 | 2026-09-21 | Exact latest version, `rust_version: 1.85`, `edition: 2024`, `license: BSD-3-Clause`, enabled feature set, and the `rand_core`/`zeroize` feature names |
| API documentation | `https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/` | `3.0.0` | 2026-09-21 | Exact API used: `SigningKey::from_bytes`, `Signer::sign`, `VerifyingKey::from_bytes`, `Verifier::verify`, `Signature::try_from`, `SECRET_KEY_LENGTH`, `PUBLIC_KEY_LENGTH` |
| Repository | `https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek` | `main` at access | 2026-09-21 | Upstream source location for the `ed25519-dalek` crate |
| License file | `https://raw.githubusercontent.com/dalek-cryptography/curve25519-dalek/main/ed25519-dalek/LICENSE` | 2017-2019 isis agora lovecruft | 2026-09-21 | The actual BSD-3-Clause text, read from the repository rather than trusted from an index summary |
| Release history API | `https://api.github.com/repos/dalek-cryptography/ed25519-dalek/releases/latest` | HTTP 404 | 2026-09-21 | Recorded: `ed25519-dalek` is a subdirectory of the `curve25519-dalek` repository and has no repository-level GitHub Releases, so crate metadata is the authoritative version source |
| RFC 8032 | `https://www.rfc-editor.org/rfc/rfc8032` | Ed25519 (EdDSA) | 2026-09-21 | The signature scheme's normative definition, including the 32-byte key and 64-byte signature lengths the code enforces |

Attempted `llms.txt` URLs that did not exist:

- `https://docs.rs/llms.txt` returned HTTP 400 and `https://crates.io/llms.txt`
  returned HTTP 404. Neither docs.rs nor crates.io publishes an AI-readable index.
- `https://raw.githubusercontent.com/dalek-cryptography/curve25519-dalek/main/llms.txt`
  returned HTTP 404. The repository publishes no `llms.txt`.
- **This is a real gap in the research gate for the Rust ecosystem**, recorded
  rather than papered over: `AGENTS.md` requires an official `llms.txt` first, and
  for a Rust crate none exists. The substitute is the versioned crate metadata
  API, the versioned `docs.rs` API documentation, and the license file read from
  upstream — all version-specific, which `llms.txt` alone would not be.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| `ed25519-dalek` | `=3.0.0` | `3.0.0` (2026-07-06) | Verified: exact pin, not a range |
| MSRV | workspace `1.98.1` | crate `rust_version: 1.85` | Verified: the crate's MSRV is below the workspace MSRV |
| Edition | workspace `2024` | crate `edition: 2024` | Verified: identical |
| License | unresolved until `OWN-001` | `BSD-3-Clause` | Reviewed; BSD-3-Clause is permissive and imposes notice retention only |
| Enabled features | `default-features = false, features = ["zeroize"]` | Documented features | Verified feature names against the crate metadata; `default` (`fast`, `zeroize`) is intentionally **not** used, so only the reviewed feature is enabled |
| Manifest format | `schema_version: 1` | JARVIS-defined | An unknown version is refused, so a future format cannot be silently misread |

**`zeroize` is enabled deliberately.** `ed25519-dalek`'s own secret-key material is
zeroized by default in the `default` feature set; because defaults are disabled,
the feature is requested explicitly rather than inherited. Losing it would mean
secret key bytes linger in memory after the signing helper returns.

**`rand_core` is deliberately not enabled.** The library does not generate keys.
Key generation needs an operating-system entropy source, which the repository
already uses directly through `getrandom`, so a second entropy path would be an
unreviewed duplicate.

## Contract

### Authentication and Authorization

- Credential type: an `Ed25519` signature over the **exact bytes** of the manifest
  document. A sidecar `manifest.json.sig` envelope carries `algorithm`, `key_id`,
  and the unpadded `base64url` signature.
- Credential placement: a detached sidecar file. The signature is not embedded in
  the manifest, because a signature cannot cover the document that contains it.
- Required scopes: none. Verification is a local, unauthenticated computation.
- Refresh/rotation behavior: not implemented. A key is trusted because it is in
  the compiled-in store; rotating a key requires shipping a binary that contains
  it. Rotation policy is `OWN-003`.
- Tenant or workspace binding: none. The trust decision is global to the build.
- Webhook signature verification: not applicable.

### Transport and Lifecycle

- Endpoint(s): none. This integration performs no network access at all; it reads
  local files. That is a deliberate boundary: a verifier that also fetches is a
  verifier whose input channel can be attacked.
- Transport(s): local filesystem only.
- Connection lifecycle: not applicable.
- Negotiation/versioning: `schema_version` on the manifest, and
  `SignatureEnvelope::algorithm`. Both fail closed on an unknown value.
- Streaming frame shape and terminal event: not applicable.
- Ordering and duplication: the manifest must list each artifact file at most
  once; a duplicate is refused, so two entries cannot describe one path with two
  different digests.
- Cancellation and timeout: not applicable; verification is bounded by the file
  sizes it is given.
- Reconnect/resume: not applicable.

### Data and Limits

- Request/response schemas: `ReleaseManifest` and `SignatureEnvelope`, both with
  `deny_unknown_fields`. Every field is bounded: version ≤ 64 bytes, build ≤ 128,
  target ≤ 64, artifacts ≤ 64 entries.
- Pagination: not applicable.
- Payload and concurrency limits: manifest ≤ 256 KiB, signature envelope ≤ 8 KiB,
  artifact hashing streamed in 64 KiB blocks so memory does not scale with the
  artifact.
- Rate-limit headers and behavior: not applicable.
- Retention and privacy: the verifier stores nothing. It does not write to the
  profile, does not log the digest of an artifact it refused, and does not copy
  artifact content anywhere. The manifest is not a secret, but it is untrusted
  input and is never echoed in a rejection.
- Data residency: not applicable.
- Cost assumptions: none. No service is called.

### Errors and Retries

| Condition | Signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Modified artifact byte | `jarvis.release_digest_mismatch` | No | Refuse; reporting the mismatch is the whole purpose |
| Truncated artifact | `jarvis.release_size_mismatch` | No | Refuse before hashing, so a bad download is caught cheaply |
| Rewritten manifest | `jarvis.release_signature_mismatch` | No | Refuse; transport security alone cannot detect this |
| Signature from an untrusted key | `jarvis.release_key_unknown` | No | Refuse; distinguish "untrusted" from "invalid" |
| Unknown signature algorithm | `jarvis.release_algorithm_unsupported` | No | Fail closed rather than ignore the field |
| Malformed signature value | `jarvis.release_signature_invalid` | No | Refuse; never truncate a signature into a shorter one that verifies |
| Manifest from a newer schema | `jarvis.release_schema_unsupported` | No | Refuse rather than guess at a future format |
| Artifact unreadable locally | `jarvis.release_artifact_unreadable` | Yes | The only retryable class: a transient local filesystem condition is not tampering |
| Artifact absent | `jarvis.release_artifact_missing` | No | Refuse; an incomplete release is not a valid release |

## Security Analysis

- Trust boundaries: the manifest, the signature envelope, and every artifact name
  are untrusted input. The artifact *file name* is a filesystem-affecting value
  taken from the document, so it is validated as a plain file name before it is
  joined to a directory.
- Prompt-injection exposure: none. The integration never reaches a model and
  contains no natural-language parsing.
- Secret leakage paths: the signing helper takes a secret key, so it is confined
  to an example binary and tests. The production path never handles a private key.
  The committed test key is the only private key in the repository and is
  documented as having no trust.
- SSRF or callback risks: none; no network access exists on this path.
- Tool side effects: none. `verify-release` only reads.
- Required approvals: none at runtime. **Publishing** a release is owner-controlled
  and blocked by `OWN-003`/`OWN-004`.
- Redaction rules: rejection messages never echo document contents. Error codes are
  stable and namespaced under `jarvis.release_*`.
- Sandbox or network policy: not applicable; the operation is read-only over
  caller-named local paths.
- Abuse cases considered:
  - **Rewrite the digest beside the artifact** — defeated by the signature.
  - **Supply your own signing key** — defeated structurally: no `--key` argument
    exists anywhere in the CLI, because a key the caller supplies cannot be a
    trust anchor.
  - **Algorithm downgrade to a weaker scheme** — defeated by asserting the
    algorithm field equals `ed25519`.
  - **Escape the artifact directory via a traversal name** — defeated by
    `is_plain_file_name`, which requires an exact expected file name that is
    canonical and contains no traversal or absolute-path components.
  - **Overflow the verifier with a huge manifest** — defeated by the size bound
    applied before parsing.
  - **Confuse a test-signed release with a real one** — mitigated by the CLI
    printing the non-production test key id and an explicit warning on every run,
    and by the CI build identity being prefixed `non-production-test/`.
  - **Present a truncated signature** — defeated because base64url decoding returns
    `None` on any partial group rather than a shorter byte string.

## Normalization Map

| Provider concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| `ed25519_dalek::Signature` | JARVIS signature bytes, unpadded `base64url` | Total; only the 64-byte encoding crosses the boundary |
| `ed25519_dalek::VerifyingKey` | Trust-store entry keyed by a JARVIS `key_id` | Loss: the library has no key identifier concept, so JARVIS owns naming and lookup |
| `ed25519_dalek::SignatureError` | `ReleaseError::SignatureMismatch` / `SignatureInvalid` | Normalized to one of two stable codes; the library's error type never leaves the module |

No `ed25519-dalek` type appears in a JARVIS public contract, a persisted format, or
a CLI surface.

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| RS-C001 | Flipping one byte of a listed artifact makes verification fail with a digest mismatch | VERIFIED by test and live run | `an_altered_artifact_byte_is_rejected`; live `jarvis verify-release` returned `jarvis.release_digest_mismatch`, exit 1 | A one-byte modification still verifies |
| RS-C002 | Rewriting a digest inside the manifest invalidates the signature | VERIFIED by test and live run | `an_altered_manifest_body_invalidates_the_signature`; live run returned `jarvis.release_signature_mismatch` | A rewritten manifest still verifies |
| RS-C003 | A signature naming a key outside the compiled-in store is refused as unknown | VERIFIED by test and live run | `a_signature_from_an_untrusted_key_is_refused_as_unknown_key`; live run returned `jarvis.release_key_unknown` | A foreign key is accepted, or reported as merely invalid |
| RS-C004 | Signer and verifier agree on the exact signed bytes across a write and read | VERIFIED by test | `the_same_manifest_bytes_sign_and_verify_across_a_write` | A written-then-read manifest no longer verifies |
| RS-C005 | The CLI cannot be told which key to trust | VERIFIED by test | `verify_release_requires_a_manifest_and_accepts_no_signing_key` asserts `--key`, `--public-key`, and `--trust` all fail to parse | Any of those spellings parses |
| RS-C006 | A traversal or absolute artifact name is refused | VERIFIED by test | `traversal_and_path_shaped_artifact_names_are_refused` | `../escape.zip` or `C:\windows\evil.exe` validates |
| RS-C007 | The committed test key has no trust outside the built-in store | VERIFIED by inspection and test | `the_builtin_store_trusts_only_the_non_production_test_key` asserts store length 1 | A second key appears in `TrustStore::builtin` |
| RS-C008 | Production signing is not implemented and cannot be mistaken for implemented | VERIFIED by inspection | `OWN-003` remains `BLOCKED` in `TODO.md`; the CLI warns on every run | A production key or identity appears in the tree |
| RS-C009 | The exact pinned version resolves and its license is BSD-3-Clause | VERIFIED | `cargo tree` shows `ed25519-dalek v3.0.0`; license text read from upstream | Resolved version differs, or the license is not permissive |

## Test Plan

### Deterministic Tests

- [x] Schema and normalization — `ReleaseManifest::parse`/`validate`, unknown
  fields, unsupported schema version, bounded sizes.
- [x] Policy and scope enforcement — the built-in trust store, unknown key,
  algorithm downgrade, and the CLI's refusal of a caller-supplied key.
- [x] Error mapping — every `ReleaseError` variant has a unique namespaced code,
  and retryability is asserted per class.
- [x] Retry/idempotency behavior — deterministic mismatches are asserted
  non-retryable; only a local read failure is retryable.
- [x] Redaction — rejection messages carry no document content; asserted by
  constructing errors from fixed inputs.
- [x] Base64url round trip across every length remainder class, and rejection of
  padding and non-alphabet bytes.

### Contract Fixtures

- [x] Real bytes, not synthetic: the release journey signs and verifies the
  **actual built `jarvis` and `jarvisd` binaries**, so the digests describe real
  artifacts rather than invented content.
- [x] Fixture provenance and capture date recorded — generated on demand by
  `scripts/release-verify-smoke.mjs`, so there is no stale checked-in fixture to
  drift.
- [x] Malformed and forward-compatible payloads covered — unknown field, unknown
  schema version, unknown algorithm, malformed signature, oversized document.

### Gated Live Tests

- [x] Verification of a genuine signed release on the authoring host: exit 0, with
  both artifacts reported as matching.
- [x] Tampered artifact: exit 1 with a digest mismatch.
- [x] Tampered manifest: exit 1 with a signature mismatch.
- [x] Untrusted key: exit 1 with an unknown-key error.
- [x] Restoration back to the genuine bytes verifies again, which is what makes
  the three failures attributable to the tampering rather than to a broken fixture.
- [ ] Native execution on all five tier-1 lanes: the journey is wired into
  `Native targets`, but that workflow's result is not observable from the
  authoring host. The lane produced zero jobs on every run until a YAML parse
  error in `native.yml` was fixed, so this is verified locally and against a Linux
  container instead; the first post-fix run is the remaining evidence.

## Operational Readiness

- [x] Health probe — not applicable; this is a one-shot read-only command.
- [x] Safe diagnostics — the command prints the manifest, signature, artifact
  directory, trust-store size, and every verified artifact, and prints the reason
  and code for a failure.
- [x] Setup and reauthentication — not applicable.
- [x] Disable/unload — delete the workflow step or the script; the library is
  inert unless called.
- [x] Migration — `schema_version` and `algorithm` both fail closed, so an old
  binary refuses a newer document rather than misreading it.
- [x] Credential rotation — **not implemented**, and stated as such: rotation
  requires shipping a binary with a new trust store and is owned by `OWN-003`.
- [x] Provider outage behavior — not applicable; no provider is contacted.
- [ ] Operator runbook — the operator procedure is "read the printed code and
  reason" in `docs/operations/ci-gates.md`; a full release runbook belongs with
  `FND-012`, which owns install, update, and rollback.

## Open Questions

- **Production signing identity and custody are unresolved (`OWN-003`).** Until
  then the built-in store contains only the non-production test key, and every
  verification says so. No production key material exists in this repository, in
  CI variables, or in any artifact.
- **Detached signature versus a transparency log.** This slice uses a sidecar
  signature, which proves authenticity but not *freshness*: it cannot detect a
  validly signed but superseded release being replayed. Rollback defense needs
  versioned metadata (TUF-style) and belongs with `FND-012`'s update channel.
- **Action pinning by commit SHA.** A workflow that handles signing must pin by
  full SHA rather than a mutable major tag. This journey signs with a
  **non-production test key and publishes nothing**, so it inherits the read-only
  gates' `@v7`-style pins; the moment a publication step is added, the pins must
  change and this note must be refreshed with the chosen SHA.
- **Whether a JARVIS-specific signature format is worth keeping** versus adopting
  an existing release-metadata format such as TUF or in-toto. Deferred: those add
  a specification and a toolchain, and the immediate need is one provable
  verification path, not a supply-chain framework.

## Change Log

| Date | Change | Why |
| --- | --- | --- |
| 2026-09-21 | Initial research; `ed25519-dalek =3.0.0` selected; manifest and signature formats defined; `jarvis verify-release` and the release journey implemented | `FND-011`: release artifacts, checksums, isolated test signatures, and consumer-side rejection of modified bytes did not exist. `OWN-003` blocks production signing, so an isolated non-production test key was used to make the mechanism provable without inventing a production identity. |
