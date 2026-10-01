# JARVIS Implementation Backlog

Status: ACCEPTED
Tracking state: ACTIVE
Last updated: 2026-09-21

Checkboxes describe repository state, not aspiration. An item is checked only
when its acceptance evidence exists and required validation passes.

## Status Key

- `[ ]` ready or pending
- `[~]` partial; not complete
- `[x]` complete and verified
- `BLOCKED` has a named dependency or decision

IDs are stable. Do not renumber completed work.

## Milestone 0: Specification

- [x] `DOC-001` Create root AI instructions with mandatory upstream research gate.
  Evidence: `AGENTS.md` and local Markdown-link validation.
- [x] `DOC-002` Define product mission, v1 scope, requirements, and non-goals.
  Evidence: `PRODUCT.md`.
- [x] `DOC-003` Define ordered milestones and exit gates.
  Evidence: `ROADMAP.md`.
- [x] `DOC-004` Create canonical implementation-agent prompt.
  Evidence: `MASTER_BUILD_PROMPT.md`.
- [x] `DOC-005` Create integration evidence policy and template.
  Evidence: `docs/research/integration-research-policy.md`, template, manifest,
  dependency ledger, and positive/negative validator runs.
- [x] `DOC-006` Publish architecture index and all bootstrap architecture docs.
  Evidence: `docs/architecture/README.md` indexes all owning architecture areas;
  the documentation validator requires every file.
- [x] `DOC-007` Accept bootstrap ADRs for core boundaries.
  Evidence: `docs/adr/README.md` indexes ten `ACCEPTED` ADRs.
- [x] `DOC-008` Publish runtime, tool, plugin, event, approval, local API, and
  voice-edge contracts.
  Evidence: `docs/contracts/README.md` and validator-required contract files.
- [x] `DOC-009` Publish conceptual schema and migration strategy.
  Evidence: `docs/data/schema.md`, `docs/data/migrations.md`, and retention rules.
- [x] `DOC-010` Publish threat model and abuse cases.
  Evidence: `docs/security/threat-model.md` and security documentation index.
- [x] `DOC-011` Publish testing strategy and full acceptance matrix.
  Evidence: `docs/testing/strategy.md` and the validator-counted stable scenarios
  in `docs/testing/acceptance.md`.
- [x] `DOC-012` Publish upstream repository study and official source registry.
  Evidence: `docs/research/upstream-projects.md` and `source-registry.md`.
- [x] `DOC-013` Create MCP, ElevenLabs, and Tauri evidence notes.
  Evidence: three accepted architecture notes indexed as `ARCHITECTURE_ONLY` in
  `docs/research/evidence-manifest.json`; implementation remains gated.
- [x] `DOC-014` Map every v1 requirement to milestone, contract, TODO, and test.
  Evidence: `docs/planning/traceability.md`; validator proves 69 unique rows and
  validates every row's linked owner, TODO, and acceptance IDs.
- [x] `DOC-015` Run local-link and documentation consistency checks.
  Evidence: `node --check scripts/validate-docs.mjs`,
  `node --test scripts/validate-docs.test.mjs`, and
  `node scripts/validate-docs.mjs` pass; mutation tests prove changed-path and
  empty-evidence cases fail closed.
  - **The consistency check ran one way only, and the missing direction found a real gap** (added
    2026-09-30; folded in rather than given a new id, because Milestone 0's `DOC-001`..`DOC-015` is a
    closed set the validator enforces — a sixteenth item is refused, and that refusal is correct).
    `traceabilityRowErrors` requires a row to name an `ACC-` that exists; nothing required a
    *documented* scenario to be named by a row. So `ACC-007` — "Configuration Compatibility", which
    `FND-004` implements — was documented in `docs/testing/acceptance.md` and owned by no requirement.
    Nothing schedules a scenario no row names, and it reads as covered because it is written down.
  - **The converse check was written as a throwaway script first, which is why the range handling
    exists.** The first version reported `ACC-071` as unowned; it is covered by `NFR-SEC-001`'s
    "`ACC-070` through `ACC-073`", whose interior ids appear nowhere literally. Without range expansion
    the check would have been a false-positive generator, and its first reader would have "fixed" a row
    that was already correct. Both spellings the table uses (`through`, `to`) expand.
  - **`unownedAcceptanceScenarios` is exported and unit-tested** (validator tests now 20, +2), with the
    range case asserted in both directions — one orphan reported against a table naming two of three,
    and an empty result when a range covers everything — so the check cannot be satisfied by always
    reporting. **Falsified by removing `ACC-007` from the `NFR-COMP-001` row**: the validator fails with
    "acceptance scenarios with no owning requirement: ACC-007". Restored. Now 80 requirements, 80 rows,
    67 scenarios, 0 unowned.

## Owner-Controlled Public Release Gates

These items block public promotion, not local implementation or isolated CI
test-signing. An AI agent must not invent the decision or resource. See
[release readiness](docs/operations/release-readiness.md).

- [ ] `OWN-001` BLOCKED: project owner selects the license and contribution
  terms; publish the license, dependency policy, and notice rules.
- [ ] `OWN-002` BLOCKED: project owner establishes and tests a private security
  reporting channel with named response ownership.
- [ ] `OWN-003` BLOCKED: project owner and Release establish production signing
  identities, custody, rotation, backup, and compromise recovery.
- [ ] `OWN-004` BLOCKED: project owner approves the release domain, channels,
  immutable metadata location, rollback, and revocation publication path.
- [ ] `OWN-005` BLOCKED: project owner approves privacy/telemetry defaults,
  retention disclosures, and required legal review.

## Milestone 1: Foundation

Dependencies: all Milestone 0 exit criteria.

- [x] `FND-000` Research the exact Rust toolchain, targets, crates, SQLite
  behavior, OS facilities, licenses, and versions proposed for Foundation;
  record current official evidence before dependency selection.
  Evidence: `docs/research/integrations/rust-foundation.md`, implementation-
  ready manifest metadata, official source/version/license review, and passing
  documentation gate tests.
- [x] `FND-001` Create Cargo workspace, pinned toolchain, workspace lint policy,
  dependency policy, and minimal crate boundaries. Depends on `FND-000`.
  Evidence: Rust/Cargo `1.98.1`, edition 2024, resolver 3, seven minimal
  packages, dependency-free domain, and `publish = false`; format, Clippy,
  tests, dependency-tree checks, and compile checks for all five supported
  targets pass. Those compile checks were valid for the dependency-light
  workspace as it stood here; `FND-006` later added bundled SQLite, which needs
  a native C toolchain per target, so per-target proof now comes from native CI.
  Native packaged/runtime journeys remain assigned to `FND-010`.
- [x] `FND-002` Implement typed IDs, clock, cancellation, correlation, and domain
  error primitives with serialization tests.
  Evidence: `jarvis-domain` typed lowercase-UUIDv7 IDs, RFC 3339 `Z` time,
  injected `Clock`/`IdGenerator` ports, and coded errors; `jarvis-application`
  cancellation scopes and server-derived `RequestContext`; `jarvis-infrastructure`
  system clock and UUIDv7 generator adapters. ID and time serialization tests
  reject non-canonical lookalikes, and cancellation propagation/race tests cover
  parent/child behavior and cancel-safe `cancelled()`. `jiff 0.2.37` is recorded
  in the Foundation evidence note; format, Clippy, and tests pass, and the
  domain crate depends only on `jiff`, `serde`, `thiserror`, and `uuid`.
- [~] `FND-003` Implement platform config/data/cache/log/runtime path resolver with
  permissions and migration tests on Windows, macOS, and Linux.
  Evidence (PARTIAL): `jarvis-infrastructure` resolves standard and portable
  profiles from `directories 6.0.0`, maps mutable state to local (non-roaming)
  paths, derives runtime/state from the local data root on Windows/macOS where
  XDG-style dirs are absent, rejects rooted/absolute/`..`/separator/drive
  components, and verifies profile containment lexically. Unix directories are
  created `0o700` and files `0o600` and the mode is queried back, so a
  group/other-accessible directory is reported as `jarvis.unsafe_permissions`.
  10 tests pass on Windows; the Unix permission tests exist and typecheck for
  Linux but have not been executed (no Linux runtime on the authoring host).
  Not done: explicit Windows DACL write/query is deferred pending an ADR,
  because it requires `unsafe` FFI that every crate forbids; native execution of
  the Unix tests belongs to `FND-010` CI. Config *schema migration* tests remain
  with `FND-004`.
- [x] `FND-004` Implement layered configuration with schema, environment overrides,
  secret references, atomic writes, and version migration.
  Evidence: `jarvis-infrastructure::config` implements defaults -> file ->
  environment allowlist -> command-line precedence (order asserted end to end),
  a versioned TOML schema with `deny_unknown_fields` at every level, an explicit
  three-key environment allowlist that reports non-allowlisted `JARVIS_*`
  variables as ignored, secret *references* (`env:JARVIS_*` only) that can never
  hold a value, a bounded 64 KiB read, and an atomic write (owner-only temp file,
  `create_new`, write, `sync_all`, rename, directory flush). An unsupported
  `schema_version` is refused and the file is left byte-identical; a raw secret
  pasted where a reference belongs fails to parse and never appears in text,
  `Debug`, or the error. 26 config tests pass; format, Clippy, and the full
  workspace suite pass, and the workspace compiled for all five targets at that
  time (cross-target compilation from one host stopped being possible once
  `FND-006` added bundled SQLite, which needs a native C toolchain per target).
  `toml::Value` is avoided because it needs the `unbounded` feature. Not done:
  crash-injection under an interrupted write and a link-swap fixture (deferred to
  `FND-012` failure testing).
- [x] `FND-005` Implement structured tracing, redaction, rotating local logs, and a
  test that seeded secrets never appear in logs or errors.
  Evidence: `jarvis-observability` installs a newline-delimited JSON file sink via
  `tracing-subscriber` with an `env-filter` directive, a time-based
  `tracing-appender` rotation with a retained-file bound, and a bounded
  `lossy(false)` queue whose dropped-line counter is exposed so degraded
  observability is visible. Redaction is applied at the **writer**, so a format
  change cannot bypass it: registered values are removed wherever they appear
  (the guarantee, proved by a seeded canary test), while authorization headers,
  `key=value` credential pairs, and URL userinfo are covered best-effort. Control
  characters and ANSI escapes are stripped, and an embedded newline cannot forge
  a second record. Values under 8 bytes are refused at registration rather than
  over-redacting. 21 tests pass; `tracing-appender` is pinned exactly to the
  reviewed `0.2.4` after `^0.2.4` resolved to an unreviewed `0.2.5`. Not done:
  console-format parity and JARVIS-owned log retention/cleanup (later slices);
  the canary scan currently covers the file sink, not yet errors, diagnostics,
  or support bundles (`ACC-070`).
- [x] `FND-006` Implement SQLite connection, migrations, migration lock, health,
  integrity check, backup, and restore.
  Evidence: `jarvis-infrastructure::storage` opens SQLite with the reviewed
  profile (bundled library, foreign keys on, WAL, synchronous FULL,
  `trusted_schema` OFF, 5s busy timeout, statement logging off) and re-reads each
  safety pragma instead of assuming it applied. The connection refuses a library
  below `3.51.3` by numeric comparison, since a string compare ranks `3.9` above
  `3.51`. Migrations are embedded from `migrations/sqlite` with a build-script
  rerun guard; `has_pending` distinguishes "migrate safely" from "report not
  ready", and a checksum mismatch is never ignored. A `schema_version` record
  makes a newer-written database refuse rather than modify itself.
  `integrity_check` **and** `foreign_key_check` both run, because a
  structurally sound file can still hold dangling references. Locks are scoped
  and expiring so a crashed holder cannot block maintenance forever, and the
  release is ownership-checked. Backup uses the online backup API (the three
  files are one state), verifies the copy before reporting success, and restore
  verifies before overwriting so a corrupt backup cannot destroy the live
  database. 44 storage tests pass; the Foundation note's previously open
  `libsqlite3-sys`/SQLite runtime gate item is now closed. Not done: upgrade from
  a prior supported schema, checksum-drift and `BUSY`/`FULL` injection,
  corrupt-database repair, checkpoint starvation, and abrupt-termination
  recovery (owner: `FND-012` failure testing and native CI).
  Side effect: bundled SQLite needs a native C toolchain per target, so the
  workspace no longer cross-compiles from one host; per-target proof moves to
  native CI (`FND-010`).
- [x] `FND-007` Implement `jarvisd` startup, graceful drain, health, readiness,
  single-instance lock, and authenticated local transport.
  Evidence: `jarvis-protocol` defines the versioned discovery file and error
  envelope; `jarvis-infrastructure` adds single-instance ownership (a held
  exclusive lock, released by the operating system on process exit, with PID text
  treated as diagnostic only), atomic discovery publication with a read-back
  validation and ownership-checked removal, 32-byte `getrandom` credentials
  stored only as SHA-256 verifiers compared with `subtle`, and an axum surface
  with global 64 KiB request limiting. Startup is a fixed fail-closed order
  (lock, database, migrate, bind loopback, publish, ready), so readiness can
  never be true while migrations are pending or the schema is unsupported.
  Liveness and readiness are separate flags, `/health/*` exposes only a status
  token, every `/api/v1` route requires a credential and an API major, browser
  `Origin` is rejected on all routes, and unknown/wrong/revoked credentials
  return one indistinguishable response. **A gap found and closed later
  (2026-09-22):** the same contract's required test 3 also names hostile `Host`
  and forwarding headers, and only `Origin` was implemented — so a request
  addressed to `localhost`, another `127.0.0.0/8` address, a foreign port, or a
  DNS name resolving to loopback reached the control surface. Binding is not an
  address filter, so `ApiState` now carries the authority the daemon **actually
  bound** and the `Host` is validated against it; `forwarded`,
  `x-forwarded-host`, `x-forwarded-proto`, and `x-real-ip` are refused outright
  because no proxy is trusted in local mode. A request with **no** `Host` is
  refused, and one with **two** is refused as well — that case was found while
  writing the tests, when a helper that appended a second `Host` was accepted
  because the first was valid, which is precisely the request-smuggling shape of
  two parties disagreeing about which header is authoritative. 50 new tests (175
  workspace-wide) pass
  across lock contention, discovery validation and unsafe-authority rejection,
  credential generation/verification/revocation and enrollment round trip,
  health/readiness, version negotiation, origin rejection, drain, and
  startup-failure paths. `jarvisd` builds and wires this into a composition root
  with bounded drain on Ctrl-C/SIGTERM. Not done: the serve loop is not yet bound
  to the router because the run resources it needs arrive with Brain;
  service-manager facilities are `FND-009`; the OS credential store is replaced
  by an owner-only file until the keyring slice (`FND-008`), which is recorded in
  the Foundation evidence note rather than left implicit. The `Host` and
  forwarding rejections are proven both at the router and over a **real socket**
  in `tests/daemon_serving.rs`, because a router test cannot show that a real
  client's header is what the control actually sees. **Two further contract gaps
  found by testing the claims the docs already made, 2026-09-22:** an unknown route
  returned `axum`'s default bare `404` with an **empty body**, so the contract's
  "unknown routes return the common error envelope" was false — a client got a
  status it could see and nothing it could parse — and the existing test passed
  because it asserted only the status; and the request-body limit produced a `413`
  whose body was `tower_http`'s plain text rather than the envelope, breaking both
  the envelope rule and the `application/json` rule for `/api/v1`. Both are fixed:
  the fallback returns `resource.not_found`, and the limit is a middleware that
  returns `request.too_large` as JSON. Moving the limit to the **outermost** layer
  was part of the fix, so an oversized body is reported as `request.too_large`
  rather than as whatever credential error the request would otherwise have hit;
  the layer order is now documented as part of the contract because it decides which
  error a caller sees.
- [x] `FND-008` Implement `jarvis status`, `config`, `service`, `logs`, and `doctor`.
  Evidence: `jarvis` parses its subcommands with `try_parse` (a typo is a typed
  error, never a process exit) and reaches the daemon through the published
  discovery file plus the enrolled local credential. `status` presents only the
  contract-shaped fields it parsed, so arbitrary daemon text cannot reach the
  operator's terminal. `config` prints secret *references* and never values;
  `logs` lists names and sizes only; `service` reports the platform backend, the
  registration state, and the exact install preview; `doctor` runs deterministic
  checks for profile directories, database integrity, SQLite version, schema
  compatibility, daemon reachability, credential presence, and service
  registration, separating blocking findings from warnings and exiting `1` when
  the operator must act. The Foundation HTTP
  client is a bounded minimal loopback exchange that re-validates the numeric
  loopback authority at call time. 5 CLI tests plus a live end-to-end check pass:
  `jarvis status` against a real running `jarvisd` reported `state: ready` with
  the daemon's own instance id and PID. Closing the `FND-007` gap, the serve loop
  is now bound: `RunningDaemon::serve_until` drives `axum::serve` with graceful
  shutdown and then drains, proved by a real-socket integration test. Not done:
  `config` does not yet write or migrate configuration, and the keyring adapter,
  service lifecycle, and native permission proof remain outstanding.
- [~] `FND-009` Implement per-user service install/remove for systemd, launchd, and
  Windows with no elevation-dependent interactive flow. Three controllers exist
  behind one trait, and every plan is asserted exactly on every host because
  planning never executes: `systemctl --user enable`, `launchctl bootstrap
  gui/<uid>`, and `schtasks /create /sc ONLOGON /rl LIMITED /it /f`. Definitions
  are escaped for their format (systemd quoting, XML entities) in addition to
  rejecting control characters, no definition contains a secret or an environment
  block, `LaunchAgent` `Label` matches its filename and bootstrap target, and
  captured service-manager output is bounded. `jarvis service` prints the backend,
  the state, and the exact install preview naming the resolved `jarvisd`; verified
  live on Windows, where the read-only `schtasks /query` of an absent task was
  confirmed to exit non-zero and map to `not_installed`. **PARTIAL**: no service
  was installed, started, stopped, or removed on any platform, the `systemd` and
  `launchd` controllers are asserted as plans only, and no `systemctl`/`launchctl`
  invocation has been observed. Those native proofs are `FND-010`. 232 workspace
  tests pass; `fmt` and `clippy -D warnings` are clean.
- [~] `FND-010` Build clean-machine CI smoke tests for the exact tier-1 target
  matrix, service lifecycle, profile paths, and owner-only permissions.
  Evidence (PARTIAL): two workflows now exist. `CI` runs the documentation gate
  **after** the validator's own test suite (so a weakened validator cannot pass
  the gate it implements), then `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets --all-features -- -D warnings`, and `cargo test
  --workspace --all-features`. `Native targets` runs the five tier-1 targets on
  matching runners (`windows-2025`, `macos-14`, `macos-15-intel`, `ubuntu-24.04`,
  `ubuntu-24.04-arm`) with `fail-fast: false`, because this workspace does not
  cross-compile: bundled SQLite needs each target's own C toolchain. The Unix
  owner-only permission assertions, which are only typechecked on the Windows
  authoring host, are executed there explicitly via `cargo test -p
  jarvis-infrastructure paths::`. A new dependency-free
  `scripts/clean-machine-smoke.mjs` implements the `ACC-001` journey: start the
  daemon on a fresh `--profile` directory, require readiness and a clean
  `jarvis doctor`, assert the profile actually contains the credential, database,
  discovery file, and lock (asserting contents, not only exit codes), assert the
  credential is mode `0600` on Unix, stop and require a bounded drain, restart
  against the same profile, and assert `jarvis service show` names `jarvisd`. It
  passes locally on Windows, including the restart, and terminates the daemon in
  a `finally` block so a failed assertion cannot leak a process. To make any of
  this possible, profile resolution was unified in `jarvis_infrastructure::profile`
  and both binaries gained `--profile <DIR>`, which is what lets a clean run avoid
  the real profile entirely; an environment override was deliberately **not**
  added, because it would be a way to redirect durable state, credentials, and the
  discovery file of an installed product. The `CI` and `Native targets` lanes were
  pinned to `actions/checkout@v7`, `actions/cache@v6`, and `actions/setup-node@v7`,
  verified against the release API after the initially written `@v5`/`@v4` pins
  proved outdated. 240 workspace tests pass; `fmt` and `clippy -D warnings` are
  clean. The workflows and journey are committed and pushed (commit `930e1d5`),
  so the lanes are executing. **PARTIAL, and worse than first recorded**: the CI
  result was read for the first time on 2026-09-21 and **every one of the 12 runs
  had failed**. The earlier note claimed the repository was private and the runs
  API unreadable; it is **public** and the API returns the runs without a token.
  Four real defects were found and fixed in three rounds, each only visible once
  the previous one was fixed: (1) `native.yml` had a YAML parse error (`paths::
  storage::` unquoted, where `: ` is read as a mapping separator), so GitHub
  created runs with **zero jobs** and the native lane **never executed
  anything**; (2) the same step's command was invalid regardless, because
  `cargo test` accepts exactly one positional filter; (3) Linux `clippy -D
  warnings` failed on an unused import, an unused `mut`, and a `verbose_bit_mask`
  lint — all invisible to a Windows compiler that never sees `#[cfg(unix)]` code;
  (4) the clean-machine journey's service assertion matched `ExecStart=`, a
  systemd directive, so it passed on Windows and Linux and failed on **macOS**,
  where launchd renders the path inside a `<string>` element. It now matches the
  executable path, and `scripts/service-assertion-check.mjs` asserts that against
  all three real platform renderings. That matcher is defined once in
  `scripts/daemon-assertion.mjs` and imported by both the guard and the journey,
  and the guard is **run by the `docs` lane**, because it previously existed as a
  literal duplicated in two files that nothing invoked — a guard that no lane runs
  can stop guarding without any build turning red. Its negative cases are the ones
  that matter: a client-only preview, and a description that merely mentions
  `jarvisd`, which is why the original matcher passed on Windows for the wrong
  reason. A `rust:1.98-slim-bookworm` container and
  `scripts/linux-verify.sh` now reproduce the CI environment locally, which is
  what made (3) findable. **Resolved**: commit `7a7d61d` is green — `CI` success
  and `Native targets` success with all five tier-1 jobs green, the first fully
  passing CI in the project's history. No CI has verified a packaged artifact,
  because no
  installer exists yet (that is `FND-011`/`FND-012`), and real per-user service
  registration is still asserted as a plan rather than performed anywhere. Native
  service lifecycle proof remains the other half of this item.
- [~] `FND-011` Build native release matrix, checksums, isolated test signatures,
  SBOM, and provenance attestations. Production signing depends on `OWN-003`.
  Evidence (PARTIAL): release authenticity now exists and is **proven to
  falsify**. `jarvis_infrastructure::release` verifies a detached `Ed25519`
  signature over the manifest's exact bytes and then hashes every listed
  artifact, and `jarvis verify-release` exposes it as a consumer-side command.
  The trust store is compiled in, so no `--key` argument exists and a
  caller-supplied key cannot be a trust anchor; rejection distinguishes an
  unknown key, an unsupported algorithm, an unsupported schema, a malformed
  signature, a size mismatch, and a digest mismatch, because those need
  different operator responses. Verified live on real binaries: a genuine
  signed release exited `0`, a single flipped artifact byte exited `1` with
  `jarvis.release_digest_mismatch`, a rewritten manifest body exited `1` with
  `jarvis.release_signature_mismatch`, and a signature naming an untrusted key
  exited `1` with `jarvis.release_key_unknown`;
  `scripts/release-verify-smoke.mjs` asserts all of that plus **restoration**,
  which is what makes the three refusals attributable to the tampering rather
  than to a broken fixture. 23 release tests plus 13 CLI tests pass, and the
  artifact-name check refuses traversal, absolute paths, drive/ADS colons, and
  dotfiles before any filesystem access. New evidence note
  `docs/research/integrations/release-signing.md` reviews `ed25519-dalek
  =3.0.0`, whose BSD-3-Clause license was read from upstream and whose MSRV
  (`1.85`) is below the workspace `1.98.1`; the enabled feature set is exactly
  `zeroize`, with `rand_core` deliberately excluded so there is no second
  entropy path beside the existing `getrandom` use. **PARTIAL**: no SBOM is
  generated, no provenance attestation is produced, no artifact is published,
  and no production identity exists. All of that is `OWN-003`/`OWN-004`, and the
  only key in the repository is the committed **non-production** test key, whose
  use is disclosed on every `verify-release` run and whose CI build identity is
  prefixed `non-production-test/`. The signed format also cannot detect a
  validly-signed-but-superseded release being replayed, because rollback defense
  needs versioned metadata (TUF-style) and belongs with `FND-012`'s update
  channel. Native execution of the journey on the five tier-1 lanes is wired
  into `Native targets`, which is now **green on all five targets** at commit
  `7a7d61d`, so the journey is proven on native Linux, macOS (both architectures),
  and Windows rather than only on the authoring host.
- [~] `FND-012` Implement install, update, rollback, portable mode, and uninstall
  tests that preserve user data unless explicitly removed. Public promotion
  depends on `OWN-001` through `OWN-005`.
  Evidence (PARTIAL): install, update, rollback, prune, and uninstall now exist as
  `jarvis_infrastructure::install` with a plan-first CLI (`jarvis install
  status|update|rollback|uninstall`). **`plan_prune` exists but has no CLI subcommand**, and the
  sentence above listed it beside a subcommand list that omits it — corrected rather than left,
  because "prune exists with a CLI" is how a reviewer concludes `MAX_RETAINED_VERSIONS` is
  enforced somewhere. It was not: the constant was referenced by nothing, and could not be, since
  `plan_prune` removes one named version and has no way to say "too many — drop the oldest". It was
  deleted in `BRN-031`; the safety property it gestured at (a bulk prune can never remove every
  path back to a working state) is enforced by the two per-version refusals and asserted directly.
  The install root and the user profile are
  **separate roots**, and every operation refuses a path inside the profile, so
  "uninstall retains user data" is structural rather than intended: removing
  versioned program directories cannot reach the database. A version becomes
  active by replacing a pointer (a symlink on Unix; an atomically renamed
  `current.version` on Windows, because a symlink there needs elevation or
  developer mode), so a reader sees the old version or the new one, never a
  mixture, and a failed activation rewrites the pointer back. A rollback target is
  recorded only when a version actually existed to replace, and rollback refuses a
  target whose directory is gone rather than silently doing nothing. A purge is the
  one destructive action that touches user data and needs **two** flags
  (`--purge` and `--acknowledge-purge`), so a safe uninstall cannot become a purge
  by passing one argument. 21 install tests pass. Verified live by
  `scripts/install-smoke.mjs`, which installs 0.1.0, updates to 0.2.0, refuses a
  rollback to a removed version (and proves the refusal changed nothing), rolls
  back to 0.1.0 while keeping 0.2.0, uninstalls and asserts the database is present
  and **byte-identical**, refuses an unacknowledged purge, then proves an
  acknowledged purge removes the data it named. Installing also requires a
  **verified release**: `plan_install`/`plan_update` take a `VerifiedRelease`,
  which can only be built by verifying a signed manifest and every artifact, and
  `apply` re-verifies the bytes from disk immediately before staging. **Four
  findings from running it:** (1) `binary_path` appended the executable suffix, so
  a manifest name that already carried it produced `jarvisd.exe.exe` and a service
  definition pointing at nothing; (2) the first staging step wrote each file onto
  its own destination instead of copying from the verified download, which cannot
  work and would have destroyed the bytes it had just verified; (3) a version was
  accepted without a leading digit, so the directory `vv1.0` was read as version
  `v1.0`; (4) a release needed a stable installed name distinct from its
  version-named download, now recorded in the signed manifest as `name` rather
  than inferred by string matching. **Not done**: no packaged installer or archive
  (the journey stages an install directory directly), no MSI/PKG/deb/rpm, no
  service-definition refresh on update, no update channel or versioned metadata so
  there is no downgrade defense, no signature on the *installed* tree beyond the
  verified release it came from, and interrupted-install recovery is limited to
  idempotent convergence rather than a tested crash-point matrix. Those are
  `FND-011`'s publication half and the owner gates.
- [~] `FND-013` Implement bounded diagnostics collection and a reviewable,
  redactable support-bundle preview/export with secret-canary tests.
  Evidence (PARTIAL): diagnostics moved out of the `jarvis` binary into
  `jarvis_infrastructure::diagnostics`, so the checks are a pure function of a
  profile plus an injected `DiagnosticsEnvironment` and are testable without a
  running daemon, a service manager, or a clock. Findings carry a stable check
  name, a severity, a bounded fact, and fixed advice, and **warnings are counted
  separately from blocking findings** so "no blocking findings" cannot be read as
  "nothing is wrong"; a daemon that is simply not running is deliberately a
  warning, because foreground and portable use are supported. The support bundle
  is plan-first: `BundlePlan::render` prints what a bundle would contain and
  which items are optional, `resolve_exclusions` refuses an unknown name or the
  required manifest by name, and `export_bundle` materializes, renders the
  manifest from the members' digest, and writes the archive in that order, so the
  digest recorded in the manifest is guaranteed to describe the archive that is
  written. Content passes through the existing `jarvis_observability::Redactor`
  (registered-value removal is the guarantee), log tails are bounded to 256 KiB
  per file over at most 5 files and start on a record boundary, and the archive
  path of a log file is sanitized so a hostile name cannot forge a traversal.
  There is no compression dependency: `diagnostics/archive.rs` writes a
  dependency-free stored (method 0) ZIP with a fixed DOS timestamp, so the same
  input produces byte-identical output and the bytes in the archive are exactly
  the bytes that were redacted. **Verified live on real binaries**: `jarvis
  doctor` reported every check plus `no blocking findings (0 warning(s))`,
  `support-bundle` previewed without writing, and an export produced an archive
  that **.NET `ZipFile::OpenRead` successfully opened** (4 members, manifest
  included). With the live 43-character client credential appended to the real
  log, the raw file contained it and the exported bundle did **not** — the member
  read `presented credential [REDACTED] while enrolling` — so the canary holds
  through the whole pipeline, not only in the redactor's own tests. 275 workspace
  tests pass (34 new); `fmt` and `clippy -D warnings` are clean. The `Cargo.toml`
  change adds only the already-reviewed `jarvis-observability` crate, which is
  why the dependency evidence note still applies. **A defect found by running
  `doctor` rather than by reading it, 2026-09-22:** the `daemon` check inferred
  reachability from the parsed **discovery file**, which outlives an unclean kill,
  so a real report contained both `warn daemon state: jarvis.stale_discovery` and
  `ok daemon: running (instance …)`. The two findings contradicted each other and
  the confident-sounding one was false. Reachability is now **probed** on the
  daemon's own unauthenticated `GET /health/live` route through a
  `DaemonReachability` port, and the answer is three-state rather than a boolean:
  `jarvis.daemon_live`, `jarvis.daemon_unreachable`, and `jarvis.port_conflict`,
  which is `ACC-003`'s port fault and was previously unreportable because any
  listener at all satisfied the check. Verified live on the built binaries: a live
  daemon reports `ok daemon: running`, an unclearly-killed one reports
  `jarvis.daemon_unreachable` with no contradictory `running` line, and a real
  foreign listener holding the published port reports `jarvis.port_conflict` with
  advice that is not the "start the daemon" text. **A second `ACC-003` fault
  closed the same way:** the `service` check no longer stops at the registration
  state. `jarvis_infrastructure::service::path_drift` reads the **installed** unit
  or plist and compares it against the executable this build would register, so a
  service that survived an update pointing at a moved binary is reported as
  `jarvis.service_path_drift` with both paths instead of as `ok service:
  installed`. The comparison uses canonical paths when both exist, so a symlinked
  equivalent is not a false drift, and falls back to the literal paths when the
  registered binary is gone — which is the fault being detected. A definition that
  exists but cannot be read is a fault, never a match, because "unknown" must not
  read as "fine". Detection is **systemd and launchd only**: a Windows task is
  defined by its command line and exposed solely through `schtasks /query /xml`, so
  there is no definition file to read, and the check reports the registration state
  it verified rather than a path claim it did not.
  `scripts/clean-machine-smoke.mjs` asserts the probe on every tier-1 target.
  **Not done**: the bundle does not yet
  summarize traces, metrics, subsystem health, runtime/plugin inventory, or
  schema/migration state, because the model, runtime, tool, connector, workflow,
  and voice subsystems that would produce them do not exist before Milestones 2
  through 8, so `ACC-078`'s "seed diagnostics across every boundary" is only
  satisfied for the daemon, storage, configuration, and service boundaries that
  exist today; there is no redaction self-test command, no attachment path for a
  user-selected run, and no retention/cleanup of previously exported bundles.
  Repair plans are `FND-014`.
- [~] `FND-014` Implement previewed and confirmed repair plans for stale locks,
  service definitions, permissions, config, and recoverable storage faults with
  backup, rollback, and verified postconditions.
  Evidence (PARTIAL): `jarvis_infrastructure::diagnostics::repair` implements
  repair as a plan value first, so every decision is previewable and assertable on
  any host. A plan carries a diagnosis, the exact actions with the path each
  touches, and a `destructive` flag; `apply` requires explicit `--confirm` and
  refuses an unconfirmed plan, a path outside the profile, a path that would remove
  user data, and a plan it cannot verify. It checks the postcondition **before**
  applying (so a plan whose outcome already holds is refused as
  `AlreadySatisfied` rather than "succeeding" at doing nothing) and **again**
  afterwards, rolling back every action that recorded an undo step. A failed
  rollback is reported, not swallowed. `jarvis repair` previews by default and
  prints the findings it will *not* touch, so "nothing to repair" cannot be read as
  "everything is fine". 41 repair/diagnostics tests plus live end-to-end runs:
  on a real profile `repair` created the five missing directories and verified the
  postcondition, re-running it refused, and after `taskkill /F` it removed the
  stale discovery file while leaving the benign lock file alone.
  **Three findings from this slice, each a defect the earlier behaviour hid:**
  (1) the directory check called `ensure_directories`, so it *created* what it
  reported and a missing-directory fault was unobservable and unrepairable — it now
  verifies without mutating and reports a missing directory as a warning (a fresh
  profile is supported, not broken);
  (2) the first stale-state repair targeted the lock file, but a clean drain
  releases the lock and leaves the file, so an unheld lock is the daemon's normal
  resting state and removing it would fix nothing — the authoritative condition is
  a discovery file surviving a *free* lock, because a drain unpublishes discovery;
  (3) the repair must be keyed on the lock-derived check, not on reachability,
  because the discovery file outlives an unclean kill and still parses, so
  `jarvis status` claims a dead daemon is running — verified live, where the
  surviving discovery file made reachability report `ok` while the lock check
  caught the stale state. 302 workspace tests pass; `fmt` and
  `clippy -D warnings` are clean. `scripts/clean-machine-smoke.mjs` now asserts
  convergence rather than a platform-dependent pre-state, because Windows cannot
  deliver a graceful stop through `child.kill`, so whether a stopped daemon leaves
  a stale discovery file differs by platform for a reason that is not a defect.
  **Not done**: service-definition drift, configuration migration, and
  permission/ACL repair on Windows; recoverable storage faults are deliberately
  excluded, because repairing corrupt data is a restore (`FND-006`) and a repair
  that rewrote it would be deleting user data; and because the daemon holds no
  live lock during a `repair` run the plan is not atomic against a daemon that
  starts mid-apply. `ACC-003`'s port and service-path faults are now **detected**
  rather than open: `jarvis.port_conflict` reports a foreign listener holding the
  published address, and the `service` check reads the installed definition and
  reports `jarvis.service_path_drift` with both paths when it names a moved
  executable (see `FND-013`). Neither has a repair plan, deliberately: a conflict
  is resolved by restarting the daemon to bind a new ephemeral port and a drift by
  reinstalling the service, and neither is a file operation this module should
  perform on the operator's behalf. The remaining open part of `ACC-003` is the
  *repair* for those faults, together with configuration migration.

## Milestone 2: Brain

Dependencies: Milestone 1 exit gate. No Brain implementation begins while a
Foundation TODO remains incomplete.

- [x] `BRN-001` Define domain model/provider capabilities and normalized model
  stream contract.
  Evidence: `jarvis-domain::model` is the typed form of the accepted
  `model-stream` and `model-data-policy` contracts plus the capability half of
  `model-gateway`. The four concepts the gateway architecture refuses to conflate
  are separate newtypes: `ProviderId`, `ModelId`, and `ModelRevision` are
  lowercase dotted slugs (owner-scoped text, since a provider is named by its
  owner), and `ModelRef` carries provider **and** model because a model ID is only
  unique inside its provider. Uppercase is rejected rather than folded, so two
  spellings cannot denote one identity. Three rules are made structural instead of
  documented: (1) every capability value carries an `Evidence` record, and only
  `VERIFIED` evidence that has not passed its inclusive `revalidate_by` day can
  satisfy a hard requirement — `DOCUMENTED`, `OBSERVED`, `INFERRED`, `UNVERIFIED`,
  and expired evidence all fail closed, because documentation describes the
  provider's current version while this repository may pin an older one; (2)
  incremental delivery is an `IncrementalDelivery` measurement of time-to-first-
  token **and** chunk spread, never a boolean, so a model that advertises
  streaming and delivers its reply in one burst is reported as a typed refusal
  rather than as a supported route — the case a `streaming: true` flag cannot
  express; (3) `RouteRequirements` reuses the same `Capability` vocabulary the
  descriptor attests and the same `Locality` the policy resolves, so a requirement
  cannot exist that no capability key could satisfy. `ModelStreamState` enforces
  the stream contract: a duplicate or reordered sequence is refused rather than
  applied (applying it fabricates a transcript), a second start and a second
  terminal are refused, and a frame arriving after the local terminal state is
  *ignored and counted* rather than treated as a fault, which is what keeps a
  correct cancellation from looking broken. A stream that ends without a terminal
  event is `StreamOutcome::Interrupted`, a distinct outcome from success, and
  `is_terminal()` is the only place that decides. `ToolArguments` exposes a raw
  string for execution **only** in the complete state, so a half-received argument
  string cannot be dispatched by a caller that forgot to check, and the completion
  payload must match the assembled deltas or the call is refused as a lost frame.
  `Usage` keeps every counter optional so unreported is never recorded as zero, and
  an unmodelled provider finish reason is preserved with its raw value instead of
  being flattened into `Stop`. `InputItems` refuses an orphaned or duplicated tool
  call **through `new` and through deserialization**, so the pairing rule holds for
  a payload that arrived over the wire and not only for a value built in-crate;
  a test falsifies the derived-deserializer shortcut. `ResolvedPolicy::merge` folds
  the six precedence layers strongest-first, where a *later* layer still narrows an
  earlier one (the case a "first deny wins" shortcut gets wrong), an empty
  allow-list means "unrestricted at this layer" rather than "nothing allowed", and
  two disjoint allow-lists are refused as contradictory instead of resolving to an
  empty set that a later layer would read as unrestricted. A cloud endpoint cannot
  be recorded as `not_applicable_local` for retention, because that reads as *more*
  careful than it is and the inconsistency is otherwise invisible.
  `EffectiveDataPolicy` has no field that could be read as a JARVIS guarantee about
  provider deletion, which a serialized-shape test asserts. Provider extensions are
  absent from the
  portable request because the contract makes them adapter-owned, and a requested
  output schema is carried as bounded `JsonText` rather than parsed, so
  `jarvis-domain` keeps the dependency set its evidence note records. The seven
  `model.*` error codes the policy contract fixes are exposed verbatim, and a test
  asserts each string so a rename cannot silently change a client-visible code.
  61 new domain tests (that crate goes 19 -> 80; 456 workspace-wide); `fmt` and
  `clippy -D warnings` are clean.
  **Not done**: this is the contract layer only. No adapter, no routing engine, no
  persistence, and no provider call exists yet — the OpenAI-compatible adapter is
  `BRN-003` and is blocked on its `REQUIRED` evidence note, the scripted provider
  is `BRN-002`, repositories are `BRN-004`, and the measured-per-model capability
  inventory that `NFR-VOI-002` and `BRN-011` need is not collected.
- [x] `BRN-002` Implement deterministic scripted model provider for tests.
  Evidence: `jarvis_application::model` adds the `ModelProvider` port and the
  deterministic `ScriptedProvider`, one unit of work because the scripted provider
  is what makes the port's contract assertable without a network, a credential, or
  a paid call, and because the architecture requires the first provider
  implementation to include the fake one. The port is narrower than a provider SDK
  — an adapter maps its own protocol into normalized `ModelStreamEvent` values at
  its boundary — so nothing provider-specific reaches the application layer, which
  is what lets `ACC-015` replace an adapter without changing the run or
  conversation schema. Four properties are structural rather than documented: the
  provider **numbers frames itself, starting at 1** (0 is the stream state's
  "nothing seen yet" cursor, so a provider numbering from 0 would be refused before
  any payload was seen); frame identifiers come from the **injected `IdGenerator`**,
  never a raw UUID call, so a stream is reproducible frame-for-frame under test
  while production keeps `UUIDv7` ordering; the provider **never sleeps**, because
  a wall-clock "slow provider" makes tests slow and non-deterministic and a passing
  test would only mean the machine was fast enough — delivery timing is *measured*
  under `BRN-011` instead; and **cancellation is an input** that ends the stream
  with the normalized `call.cancelled` terminal rather than a bare stop, which
  would be recorded as an interrupted fault instead of a cancellation.
  `ProviderError` keeps a provider **refusal** apart from an **invalid request**
  (one is a decision repeating cannot change, the other a defect in JARVIS) and
  `retryable` is the single place that decides retryability, because the
  architecture gives exactly one layer ownership of each retry.
  `ScriptStep::Raw` exists so the contract's *negative* cases are scriptable at
  all, and **two of those tests found defects the positive cases could not**:
  a raw frame's call was being rewritten to the stream's own call, making "a frame
  for another call is refused" unproducible while the code appeared to support it;
  and the stream disabled its cancellation path as soon as the script merely
  *contained* a terminal, so a caller cancelling while the first frame was still
  queued received the script's `call.completed` instead of `call.cancelled` — a
  cancellation recorded as success. **A third defect was a design one that no test
  used so far could catch:** the port originally returned a concrete same-crate
  stream struct with a private frame buffer, so `ModelProvider` read as a general
  trait while no adapter in another crate could implement it — `BRN-003` would have
  had to add its own constructor inside this crate. The port now returns the
  `ModelStream` **trait**, with `AdapterStream` as the shared implementation an
  adapter uses (it observes cancellation *while waiting* for a frame, because an
  adapter that checked only between frames would hang a cancelled call whose
  provider had gone quiet, and it produces exactly one terminal, so a channel that
  closes with none ends the stream as *interrupted* rather than fabricating a
  completion). `an_adapter_can_implement_the_port_without_this_modules_internals`
  is the test that would have caught it: a port is only as general as the crates
  that can implement it, and a single in-crate implementor hides the difference.
  The contract's `settings` block, which the domain did not model, is added as
  `PortableSettings`: **thousandths rather than floats**, because a float cannot
  derive the `Eq` the module relies on and a decimal bound such as `2.0` is not
  exactly representable, so an out-of-range value is refused with
  `model.settings_invalid` rather than clamped (a clamped call runs with settings
  the caller did not choose, and the caller cannot tell the difference). The block
  is `deny_unknown_fields`, so a provider-only key inside it is a parse failure —
  the same rule as the absent extension map stated from the other direction.
  27 new application tests plus 3 domain tests (480 workspace-wide; `jarvis-domain`
  80 -> 82, `jarvis-application` 5 -> 31); `fmt` and `clippy -D warnings` are clean.
  **Not done**: the scripted provider is a test double, not a product path — no
  adapter, routing engine, capability inventory, repository, or run state machine
  exists yet, so nothing calls `ModelProvider` outside tests. `BRN-003` is blocked
  on its `REQUIRED` evidence note, `BRN-004` and `BRN-005` follow, and the measured
  per-model capability inventory that `BRN-011` and `NFR-VOI-002` need is not
  collected.
- [~] `BRN-003` Research and implement one OpenAI-compatible provider adapter.
  Evidence: **all three halves now exist** — the research gate, the adapter, and a **verified real
  provider run**.
  `crates/jarvis-infrastructure/src/model_providers/openai_compatible/` holds it in three layers,
  chosen so every provider-shaped trap is testable without a socket: `sse.rs` reassembles frames from
  a chunked byte stream (8 tests), `translate.rs` maps one chunk onto normalized events (15 tests),
  and `mod.rs` owns the socket, the HTTP exchange, chunked decoding, error mapping, and the credential
  (19 tests). `tests/openai_compatible_stream.rs` drives all of it against a **real loopback socket**
  with a hand-written server (12 tests), because the two pure layers were already covered by fixtures
  and what remained unproven was exactly the transport.
  - **Two real defects were found by the socket-level test and by nothing else.** The **chunked
    decoder handed chunk framing to the SSE parser**: its first version drained the payload while the
    reader delivered whatever was buffered, so the *next* decode read payload bytes as a hex size
    line. The symptom was `model.provider_malformed` against a perfectly healthy stream — a framing
    bug wearing a protocol error's clothes, and invisible to a unit test that fed the SSE parser
    directly. Fixed by leaving the payload buffered behind a `payload_remaining` counter, so a chunk
    larger than the caller's output buffer is delivered across several reads instead of dropped. The
    second was not a bug but a shape: `ModelProvider::open` borrows the context, request, and
    cancellation scope for the stream's lifetime, so a caller must **own** all three — a constraint
    only the out-of-crate test could make concrete.
  - **The refusal flag is deliberately NOT folded here.** The run controller's `completed_reason`
    already owns that fold and upgrades a plain `Stop` and nothing else, so folding in the translator
    as well would put one rule in two layers that could drift — and a second implementation of "did
    this model decline" is the duplicate the architecture forbids. The translator reports the
    provider's reason verbatim with `refused` beside it, which is why its own test asserts `Stop` and
    the flag rather than asserting `Refusal`.
  - **The transport is loopback-only, and the constructor refuses everything else.** The evidence
    note records the measurement behind that: `Cargo.lock` contains **no TLS implementation at all**
    on 2026-09-27, so reaching a remote host would send a credential in the clear. `ConfigError::
    NotLoopback` refuses a name, a private address, and a public address alike, and `UrlShaped`
    refuses a scheme, path, query, or userinfo — the shapes that are how a key ends up inside a base
    URL. A host is therefore parsed as an address rather than prefix-matched, so `127.0.0.1.evil`
    cannot pass.
  - **The credential is a header value and a private field.** `BearerKey` has no accessor other than
    `expose_for_header`, no derived `Debug`, and no `Clone`, which is the same construction the
    authentication module's `GeneratedCredential` uses — **no new dependency was added for it**,
    because a crate for that property would be a dependency change with its own evidence obligation
    while the property is a private field this file can establish and test. A canary test asserts the
    value cannot reach the adapter's `Debug`, any frame, a failure code, or a mapped error.
  - **A `429` is mapped by its code, not its status.** The provider's own guide states that different
    conditions share a status: an exhausted balance, an organization spend limit, a project spend
    limit, and a usage limit all arrive as `429` and **none is retryable**, because retrying cannot
    restore access. Only the rate-limit family is, and an unrecognized `429` is treated as retryable
    since the run's budget still bounds the attempts. A `403` with `insufficient_quota` is an
    exhausted balance wearing a permission status.
  - **Verification.** 1111 workspace tests (+72 across the two slices) with `fmt` and
    `clippy -D warnings` clean; both doc gates green, including `--changed-file` for every new adapter
    and composition path. `cargo doc` reports only the 7 pre-existing intra-doc-link warnings
    (4 `private_intra_doc_links`, 3 `redundant_explicit_links`), which `AGENTS.md` explicitly scopes
    out of the `-D warnings` requirement; the adapter adds none. The manifest's
    `implementation_paths` gained the **underscore** spellings
    (`crates/**/src/model_providers/openai_compatible/**`) because a Rust module directory cannot
    contain a hyphen — so the pre-existing hyphen-only globs matched no real path, and an adapter
    written at the only legal location was refused by its own gate. Found by running the gate against
    the new files rather than by reading the glob.
  - **Now composed into the daemon.** The adapter is no longer a library nothing reaches:
    `jarvis_infrastructure::model_providers::resolve` maps a configuration document plus a secret
    resolver to the provider a daemon calls, `jarvisd` composes it **before** startup, and `run_ports`
    uses it in place of the inline script. Four properties, each of which the tests hold:
    **no `[model.provider]` table means the scripted provider** (the default for a fresh install and
    every test, so a profile naming no endpoint cannot silently reach the network); **a configured
    endpoint that cannot be composed is a startup refusal, not a fallback** — the load-bearing rule,
    because falling back would let a typo route every run to a provider that answers with a fixed
    acknowledgement while the operator saw successful runs, making the misconfiguration invisible;
    **the credential is resolved once, at composition**, which for a long-lived daemon is the last
    responsible moment, so the value is held for the process's lifetime, never written to a record,
    and never rendered; and **the provider id and model names are validated with the domain's own
    identifier rule**, because both reach a persisted row and a routing decision.
  - **Three defects-or-findings the composition surfaced.** (1) **The configuration schema had to
    move 1 → 2.** A version-1 binary's unknown-field rejection would report a document naming a
    provider endpoint as a *parse* failure, when the accurate diagnostic is "written by a newer
    JARVIS" — so the version moved, and **version 1 remains readable**, because the only difference is
    an optional table and refusing it would lock an operator out of their own profile for no security
    benefit. (2) **The configuration layer must not validate the host.** Whether an endpoint is
    admissible is the adapter's loopback rule, and a second predicate here could disagree with the
    first, so an invalid endpoint fails with the **adapter's own code** from the one place that owns
    the decision. (3) **Two Operational Readiness items in the evidence note were wrong**, not merely
    unchecked: it claimed a key takes effect "on the next call" (it takes effect on the next **start**,
    because composition resolves it once) and that removing the provider yields
    `model.provider_no_route` (the run instead fails with `run.no_model_served`, because the recorded
    route names the adapter's model and the controller requires the provider to still serve it — so
    removal surfaces as a run failure rather than a quiet switch of source). Both are corrected in the
    note.
  - **The end-to-end proof is at the level the composition can fail.** `tests/daemon_provider_composition.rs`
    starts a **real daemon** over a **real socket**, points it at a **local fake OpenAI-compatible
    server**, creates a run through the **real control API**, and reads it back. It asserts the two
    things no provider-level test can see: that the fake server **received** the request (naming the
    routed model, with the credential as a header) and that the streamed answer is **durable** in the
    run's own events. **Falsified**: replacing the composition with the scripted provider fails the
    test with "the provider was called exactly once" — the server was never reached. The negative half
    is covered too: an unconfigured profile composes the scripted provider, and a configuration the
    adapter refuses stops the daemon rather than falling back.
  - **A real provider run is now verified end to end, which closes Milestone 2's exit gate.** The
    evidence note's gated live test was the last outstanding item for this TODO, and it is no longer
    `DOCUMENTED`: `jarvis ask "Reply with exactly: JARVIS OLLAMA OK"` against a daemon configured for
    **Ollama on loopback** printed `JARVIS OLLAMA OK` and exited 0, with the model's own text durable
    in the run's events. `tests/e2e/provider-smoke.mjs` is the automated form, asserting the daemon is
    wired to the configured provider rather than the scripted fallback, that the run reaches
    `completed`, that the delta carries the model's text, and that `jarvis ask` prints it. It is gated
    on a **reachable loopback endpoint** rather than on a secret — because no cloud endpoint is
    reachable from this build at all, so a key would not have helped — and a skip is **reported** as
    "nothing was proved" rather than as a pass, since a CI log that could not tell the two apart is
    the one thing a gated test must not produce. Ollama needs no credential, so the whole transport is
    exercised with no TLS and no dependency change.
  - **The real endpoint forced two additions that no fixture could reveal, and both are general
    rather than Ollama-specific.** (1) **A base path.** The completion route is `/chat/completions`
    only when the server is rooted at the origin; Ollama serves `/v1/chat/completions`, as do vLLM, LM
    Studio, and most gateways — so the adapter was posting to a path that 404s on a real server while
    every fixture passed, because a fixture's server is built to the adapter's own assumption.
    `[model.provider].base_path` is validated as a **path**, not accepted as a URL: absolute, no
    traversal, no query or fragment, no interior whitespace or control characters — which keeps the
    "a key cannot end up in the endpoint" property the host/port split exists for. (2) **A
    model-name mapping, and this is a genuine namespace problem.** A JARVIS model id is a lowercase
    dotted slug — one spelling per identity, which is what makes a routing decision, a persisted row,
    and a diagnostic comparable — while a provider names models however it likes: Ollama's
    `glm-5.3-flash:cloud`, `model@2026-01`, `org/model`. **A colon cannot appear in a JARVIS model
    id**, so the adapter could only address a model whose provider name happened to already be legal,
    which silently excluded every Ollama cloud model. `[model.provider].model_names` maps a served
    model's id to the provider's own spelling, and a mapping for a model the endpoint does not serve is
    **refused at composition** because it would never be used and the operator would believe it had
    been. The routed model is still exactly one the adapter serves; the mapping changes only how it is
    spelled on the wire. Both additions leave the default configuration byte-identical.
  - **A third defect was found by making the stream live, and the CLI's own transport was wrong for
    it.** `Body::from_stream` makes hyper emit `transfer-encoding: chunked`, and the hand-written Rust
    client read the body as if the chunk framing *were* the message. It appeared to work, which is
    what made it dangerous: every frame happened to fit in one chunk, so the size lines were skipped
    by the SSE parser as unknown lines. A frame larger than hyper's write buffer splits across chunks,
    and then a size line lands **inside a `data:` line** — the JSON loses its closing brace, the frame
    parses as nothing, and **`jarvis ask` prints a truncated answer and exits 0**. Fixed with
    `framing_of` (decided from the header, never sniffed, because a body can coincidentally begin with
    something that looks like a chunk size) and `decode_chunked` (refuses incomplete framing rather
    than returning a prefix the caller cannot tell from a complete body). **The wiring test mattered
    more than the decoder test**: with the decoder correct but never called, every unit test still
    passed, and only a socket-level test that asserts the *transport's* output caught it.
  - **A dropped tool call was found by comparing the adapter's code against its own security
    section, and it was a silent-success bug rather than a missing feature.** The adapter's evidence
    note states "tool calls appear as `tool.call.*` events and remain subject to the canonical tool
    pipeline" — but `translate.rs` *recognized* `tool_calls` and emitted **nothing**. The reasoning was
    that the tool fabric does not exist so a tool call "must not be proposed". That is backwards, and
    the controller's own code says why: it has a typed terminal refusal (`run.tools_not_implemented`)
    with **no other way to reach it**, so dropping the event did not prevent a proposal — it removed
    the only thing that made the refusal accurate. A real model that asked to call a tool produced a
    stream with no delta and no tool event, so the run could reach `completed` with an **empty
    answer**. A silent success on a dropped intent is strictly worse than a typed refusal, because the
    model asked to *do* something and JARVIS reported that it finished. The translation now emits
    `tool.call.added`, `tool.call.arguments.delta`, and `tool.call.completed`, with the completion
    carrying the **accumulated** arguments because a fragment is not parseable JSON and the fabric
    validates the whole value. Emitting is still not a grant — the events are proposals the
    deterministic layer judges. Five tests, and the old behaviour is falsified by them.
  - **Not done, and named:** **no cloud endpoint.** Everything above is loopback, because the build
    has no TLS implementation, so a cloud provider still needs its own reviewed dependency before it
    is reachable — the note records that, and the loopback-only refusal is what keeps the gap from
    being a silent plaintext credential. `OC-C004`'s `[DONE]` half stays open (Ollama's stream
    terminated on `finish_reason`, which is what the adapter relies on, but the harness does not
    inspect the sentinel). The legacy singular `function_call` delta is **not** translated, and the
    asymmetry is deliberate rather than an oversight: it carries no `index` and no `id`, so it cannot
    supply the canonical call identifier a later continuation needs, and translating it would emit a
    call with a synthesized id nothing could reference. Structured output and portable sampling
    settings are still refused rather than silently dropped, so they are missing capabilities rather
    than defects. **DO NOT COMMIT.**
  `docs/research/integrations/openai-compatible-model.md` moves the manifest entry
  from `REQUIRED` to `IMPLEMENTATION_READY`, which is the first time any path
  matching `crates/**/src/model_providers/openai-compatible/**` has been *reachable*
  rather than refused by the gate — before this, the first adapter edit was
  impossible by policy, not by difficulty. Three findings change the design rather
  than decorate it. **The target contract is Chat Completions streaming, not
  Responses.** `BRN-002`'s port maps onto `output.text.delta` / `call.completed`,
  and the repository's compatibility edge (`api-protocols.md`) names
  `/v1/chat/completions`; the Responses API is a *different* documented contract
  with a different request shape (`input`/`instructions`), a different event set
  (`response.output_text.delta`), and `store` defaulting to **true**, so the two
  pages must not be treated as one API with two spellings. The streaming-events
  page is the one that owns the frame schema, and the reference subtree has no
  working per-section `llms.txt`: `.../chat/llms.txt`, `.../chat/create.md`, and
  `.../chat/completions/methods/create.md` all 404 while
  `/api/reference/llms.txt` lists the real slugs. **A refusal arrives as HTTP 200**
  with `delta.refusal` populated, so a reader keyed on the status records a refusal
  as a finished answer — which is exactly the failure `FinishReason::Refusal`
  exists to prevent, and it is the reason that variant is not merely stylistic.
  And **abuse-monitoring retention is 30 days and is not client-side disablable**;
  the per-endpoint table records training use `No` but application-state retention
  and a 30-day monitoring window whose exclusion requires provider approval, so a
  JARVIS policy asserting `maximum_provider_retention: none_documented` against a
  cloud endpoint of this contract is false by default and the honest inventory
  value is a bounded 30-day window. Two further facts are recorded as
  `UNVERIFIED` rather than assumed: the `[DONE]` sentinel does not appear on the
  page this note cites, so the adapter terminates on the first non-null
  `finish_reason` or end-of-body and treats `[DONE]` as an ignorable sentinel, and
  the streaming-events page renders `finish_reason` as five literals plus
  "or 2 more", so an unmodelled value maps to `FinishReason::Other` rather than
  being flattened to `Stop`.
  **The transport is loopback and plaintext, and the reason is a measurement rather
  than a preference.** A lockfile inspection on 2026-09-27 found `rustls`, `ring`,
  `aws-lc-rs`, `webpki`, `native-tls`, `openssl`, `hyper-rustls`, and `tokio-rustls`
  all **absent**; only `hyper` (a transitive server component), `hyper-util`,
  `tower-http`, `http-body-util`, and `httparse` resolve. Reaching a cloud endpoint
  therefore requires a *new reviewed dependency*, which `AGENTS.md` makes its own
  evidence obligation, so shipping it as a side effect of an adapter would be the
  failure mode the dependency gate exists to prevent. `EndpointClass::Local` already
  models the local case, and the Foundation client already establishes the pattern
  and its rationale (a minimal HTTP/1.1 exchange rather than a general client
  stack), so the first slice is a real streamed provider call with no unreviewed TLS
  stack smuggled in and the cloud path left as a separately evidenced step.
  **Verification performed**: the metadata cross-check was falsified in both
  directions rather than trusted — mutating `Last verified` to disagree with the
  manifest produced `ERROR: openai-compatible-model: Last verified metadata
  mismatch` and exit 1, restoring it returned exit 0; and a changed path matching
  `crates/**/src/model_providers/openai-compatible/**` now validates at exit 0 where
  it was previously refused. `node scripts/validate-docs.mjs` reports 90 Markdown
  files and 19 evidence entries (89 -> 90, 18 -> 19), the changed-file invocation
  passes, and the 18 validator tests still pass. The note also carries 10 falsifiable
  claims and marks its Contract Fixtures and Gated Live Tests sections **unchecked**,
  with the fixture item called out because a synthetic fixture only proves the
  implementation agrees with itself.
  **Not done**: no adapter, no request builder, no SSE parser, and no gated live
  test exists. Every runtime item in the note's Operational Readiness list is
  unchecked, and the `[DONE]` question stays open until a real capture settles it.
  `BRN-011`'s per-model aggregation is therefore still blocked, and the Milestone 2
  exit gate ("a gated real-provider smoke test streams a response") is still unmet.
- [x] `BRN-005` Implement native agent state machine with explicit terminal and
  waiting states.
  Evidence: `jarvis_domain::run` (`state.rs`, `lifecycle.rs`) implements the state
  machine in `docs/architecture/agent-runtime.md`, the document that owns the run
  controller. The state set and every legal edge are the diagram **transcribed**
  rather than derived, and `RunState::allowed_targets` is asserted table-for-table
  by `every_edge_in_the_architecture_diagram_is_present`, so the code and the
  diagram cannot drift silently. `RunLifecycle` keeps the state **private**, so
  every change goes through `apply` and an edge the diagram lacks cannot be reached
  by assigning to a field. Four rules are structural: terminal states **absorb**
  (so a late worker cannot resurrect a finished run, and the local control API's
  "exactly one terminal event" rule is unreachable from below); a transition
  carries **provenance** (actor from a closed set, a bounded reason, the expected
  prior version, the instant, an optional correlation ID) and
  `RunTransitionRecord` records what was *applied* — both versions included —
  because "the state changed" is not auditable on its own; refusals are ordered
  **terminal → version → edge** so a caller gets the most specific true answer (an
  illegal edge computed from a stale view may be legal from the current state), and
  `RunVersionConflict` is the one **retryable** domain error, because re-reading and
  recomputing is exactly what stating an expected version is for; and waiting is
  **explicit and distinct from failure** (`AwaitingApproval`/`Waiting` are named
  states, so a run parked on a dependency does not look like a run that is merely
  not progressing). A mismatched `expected_from` at the right version is refused as
  well, which is the two-workers-from-one-version case a version check alone would
  let through. 27 new domain tests (that crate goes 82 -> 109; 507 workspace-wide),
  including reachability proofs that every non-terminal state can reach a terminal
  state and none has a self-edge. **Both questions this transcription surfaced are
  now resolved, and resolving them found a third, blocking defect.** The diagram gave
  `Responding` exactly one successor (`Completed`), so a provider failure or a
  cancellation *while producing the final answer* was not expressible;
  `Responding -> Failed` and `Responding -> Cancelled` are now edges, because
  producing the answer is where both actually arrive and `ACC-012`/`ACC-016` require
  them to be persistable. Reviewing that same area found the larger defect:
  **`AwaitingModel` had no edge to `Responding`**, so the native runtime's own
  instruction ("ask a model for either a final response or typed tool intent") had no
  legal completion path — a plain question-and-answer run could reach neither
  `Responding` nor `Completed` and would sit in `AwaitingModel` forever, blocking
  `BRN-007`. The edge was missing from the diagram, not the design: the
  [first vertical slice](docs/planning/first-vertical-slice.md) already names the
  minimal machine as "received, context, model, responding, completed". The original
  happy-path test did not catch it because it drove `Planning -> Responding` and so
  never visited `AwaitingModel` — **a happy-path test that avoids a state proves
  nothing about that state**, so the repository suite now drives the real path
  (`a_plain_question_and_answer_run_persists_through_to_completed`) and a failure
  while responding is asserted to persist as `failed`, never as `completed`. The
  client-visible state set in the local control API remains **coarser** than the
  controller's, with no projection function here on purpose because the architecture
  forbids domain states doubling as UI strings, so that total mapping is `BRN-007`'s
  at the API boundary. **Not done**: this is the transition layer only — the state
  machine itself owns no I/O and names no UI string. Since this entry was written the
  layer is exercised: `BRN-004` persists a run and its events and `BRN-008`'s run
  controller drives the machine to a terminal state, so `RunLifecycle` now has a
  caller. What remains outside this TODO is the client-visible projection: no
  controller state is mapped onto the wire yet, because that mapping is `BRN-007`'s at
  the API boundary. Context budgeting is `BRN-006`, and the CLI/HTTP surfaces are
  `BRN-007`.
- [x] `BRN-004` Implement durable session/message/run/model-call repositories.
  Evidence: `migrations/sqlite/000002_conversations_runs.sql` creates
  `conversations`, `messages`, `agent_runs`, `agent_steps`, `run_activity_events`,
  and `model_calls`, and raises the schema to version 2 with the minimum reader left
  at 1 because the migration is purely additive.
  `jarvis_application::repository` holds the ports and
  `jarvis_infrastructure::storage::repositories` implements them over SQLite, so the
  domain still depends on traits rather than on `SQLx` types. Three constraints do
  the work rather than decorating the schema: `agent_runs` **refuses a waiting state
  whose dependency is unset and a terminal state whose completion instant is unset**
  (mirrored in `RunWrite::is_consistent`, with a test for each direction of the
  mismatch, because a row that reads as waiting with nothing to wait for or as
  finished with no instant would look plausible); `UNIQUE(run_id, sequence)` on
  `run_activity_events` keeps "sequence increases by exactly one" true; and
  `UNIQUE(logical_call_id, attempt)` on `model_calls` makes a retry an *attempt* of
  one logical call rather than a new call, which is what the retry-ownership rule
  depends on. `RunRepository::transition` commits the state change and its activity
  event in **one transaction** — the first required atomic use case in the storage
  architecture — so a reader never sees an event for a transition that is not
  durable, or a transition with no event. Scope is a query predicate, so a run in
  another workspace is `NotFound` rather than a forbidden result, as the local
  control API requires; and a stored state the domain does not recognize is
  `Corrupted`, never "absent", because treating it as absent converts a migration
  problem into apparent data loss. **Two defect classes were found by the tests, and
  the first is why an edge check must not live in a SQL predicate:** the first
  implementation inferred a transition's legality from its `... AND state = ?`
  predicate, which proves only that the run *was* in the expected state — it
  **accepted `Received -> Responding`**, an edge the architecture diagram does not
  contain, and passed every test until one asserted the refusal. The adapter now
  reads the row inside the transaction, orders its refusals exactly as the domain
  does (terminal, then version, then edge) and asks `RunState::can_transition_to`
  rather than duplicating the table. Second, constraint failures were mapped to
  `storage.query_failed`, reporting a **caller-visible conflict as a transport
  fault** a blind retry might "fix"; they now map to `storage.conflict` across runs,
  conversations, messages, activity, and model calls. A separate domain fix in the
  same change: `SessionId`, documented as identifying a "durable conversation
  session", conflated the conversation aggregate with the identity architecture's
  **authenticated session** (the `sessions` table), so it is now `ConversationId`
  with `MessageId` added. 23 adapter tests against a **real migrated database** plus
  12 port tests and 3 domain tests (546 workspace-wide; application 31 -> 46,
  infrastructure 315 -> 339, domain 109). `fmt` and `clippy -D warnings` are clean.
  **Not done**: `agent_steps` has a table but no port or adapter, so no step is
  persisted; `model_route_decisions`/`model_data_policies`/`model_policy_exceptions`
  are not created (routing and policy are `BRN-010`); the schema-version bump has
  not been exercised as an upgrade from a **populated** version-1 database, which
  `docs/data/migrations.md` requires (`PRD-009`); no PostgreSQL implementation
  exists (`PRD-001`); and no controller, endpoint, or CLI path calls these
  repositories yet.
- [x] `BRN-006` Implement context budgeting for identity, active task, and recent
  conversation.
  Evidence: `jarvis_domain::context` implements the second half of the
  `docs/architecture/memory-context.md` retrieval pipeline as types, so
  `FR-CTX-001` ("context selection is budgeted, provenance-aware, and recorded") is
  enforced rather than restated. `ContextBudget::assemble` is the pipeline and the
  step order is the rule: **policy and scope filtering precede ranking**, because the
  architecture states that post-filtering a ranked result "leaks both recall and
  potential side channels" — a candidate over the sensitivity ceiling or past its
  validity is refused *before* it is scored, and every refusal is **counted by
  reason** rather than silently dropped; deduplication follows ranking, so the
  highest-ranked occurrence of a reference is kept rather than whichever came first
  in the caller's list; ranking is a **total order** (priority band, score, recency,
  reference) so the same set assembles identically regardless of input order; and an
  item that does not fit is **excluded, never truncated**, because a half-truncated
  message reads as a complete one to the model. `ContextManifest` records each
  inclusion's reference, source, sensitivity, token estimate, reason, and score plus
  an exclusion summary by reason, which is the architecture's "Context Manifest"
  requirement met without storing duplicate prompt text — and references content
  rather than containing it, because a manifest that embedded the text would outlive
  the retention decision that allowed it. Three properties are structural: a
  **zero-cost candidate is refused**, since it is the one way content could enter
  without consuming budget; a **non-finite score is refused**, because `NaN` makes
  the ranking comparator's order undefined; and **hidden reasoning is inexpressible
  rather than filtered**, because `CandidateSource` has no variant for model-internal
  reasoning — `FR-RUN-005` holds by construction, and a serialized-shape test asserts
  the manifest has no field that could carry it. `ContextManifest::is_consistent`
  checks its own arithmetic (included plus excluded equals offered, used tokens equal
  the included sum, used never exceeds the budget), because a manifest failing that
  describes a decision the code could not have made. 22 new domain tests (that crate
  goes 109 -> 131; 568 workspace-wide). `fmt` and `clippy -D warnings` are clean.
  **Not done**: this is candidate selection and budgeting only — nothing *retrieves*
  candidates, so lexical, semantic, and hybrid retrieval and entity resolution remain
  Milestone 4 (`MEM-003`..`MEM-006`) and the score a caller supplies is a value
  JARVIS does not yet compute; summarization/compaction is `MEM-008`; the manifest is
  not persisted, because the `context_manifests`/`context_manifest_items` tables are
  not created; and `ACC-035`'s cross-workspace candidate
  case cannot be exercised end to end until retrieval exists.
  **The pipeline now has its caller, and the last gap this entry recorded is closed.**
  `jarvis_domain::run::budget`'s `RunBudget` gained `max_context_tokens`, and
  `jarvis_application::context_assembly` is the bridge between the two halves that cannot
  see each other: the domain decides *which* references fit a budget and deliberately never
  holds content, while the content lives in stored messages that only the repository port
  can read. The run controller now builds its prompt from what **fit the budget** rather
  than from everything it read, so the request is bounded in the dimension that reaches the
  provider instead of only in message count — a window of 200 messages can exceed any
  model's window, which is exactly why this slice was written. The result is reordered so
  the objective is placed **last**: the assembly returns items in score order, and since the
  objective scores highest while a newer message outranks an older one, the naive order put
  the *oldest* turn second and broke the transcript's chronology. Reordering is safe
  precisely because every item already fit the budget — it changes presentation, not
  selection. **The untrusted-content delimiting this entry listed as "consumed by nothing"
  is now consumed:** `RetainedItem::to_input_item` delimits an item whose source is
  untrusted, driven by the domain's own `SourcePriority::is_untrusted` rather than a second
  list in the application layer. **Three defects came out of wiring it, two of them
  introduced by the wiring.** First, the objective was sent as an **empty message**: a
  retained item was modelled as a `kind` tag beside an `Option<StoredMessage>`, so the tag
  could say `Objective` while no content was present — every field's type was satisfied, the
  request was valid, and the run completed having asked the model nothing. Making the content
  part of the **variant** (`Message(StoredMessage) | Objective(String)`) makes "an item exists
  whose content is missing" unconstructible, and the test that caught it asserted the item's
  *text* rather than its kind, because a test that checks which variant was produced cannot
  detect a wrong value inside it. Second, an assembly failure left the run **stuck in
  `ContextBuilding`** — the error was propagated with `?` and no transition, so the run had no
  legal exit, the **sixth** instance of this project's missing-terminal-exit class and again
  invisible until a test asserted the run's *stored state* rather than only the returned error;
  every path out of the context stage now goes through one helper, so the rule is a single
  decision rather than a transition repeated at each site, which is how one came to be missing.
  Third, the candidate score was derived from the **slice index**, so the same conversation
  assembled differently depending on the order it was read in — the exact property the domain's
  total order exists to provide; it is now derived from the message's own durable `sequence`.
  Falsified twice: making the effective ceiling unbounded sent **20,002** estimated tokens
  instead of the 8,192 default and failed the bound test, and removing the terminal transition
  left the run in `ContextBuilding` and failed the stored-state assertion. 13 assembly tests,
  5 controller tests, and 4 budget tests. **Still not done, and stated rather than implied:**
  the manifest is not persisted (`agent_runs.context_manifest_id` stays NULL, because the
  `context_manifests` table and the selection ledger are `MEM-008`); the sensitivity ceiling is
  passed as one that admits every label JARVIS writes, because the merged data policy's
  `maximum_sensitivity` is `BRN-010` and does not exist — the manifest **records** each label,
  so the gap is visible rather than papered over, and a defaulted ceiling here would read
  exactly like a real policy while holding nothing back; and the token counts are documented
  estimates from a byte division, since counting tokens is a model-specific job no adapter
  provides yet, which is why every ceiling this milestone *enforces* is still checked against
  the provider's reported usage instead.
- [~] `BRN-007` Implement CLI chat plus HTTP/SSE streaming. **Partial**: the run
  resource surface, the CLI chat path, the live SSE follow, and the CLI's single-connection
  streaming renderer are all implemented and verified end to end; what remains named below is
  elsewhere.
  Evidence: `jarvis_application::run_service` is the orchestration — it resolves the
  conversation, claims the idempotency key, creates the run, spawns the controller, and
  records cancellation intent — and `jarvis_infrastructure::http::runs` is the thin
  boundary over it, so the handlers do nothing but resolve the authenticated scope,
  parse the command, and call the service. `jarvis-protocol::run` carries the wire
  types, so the daemon and the client share one serialization definition rather than
  each defining its own. Six endpoints now exist: `POST /api/v1/runs`,
  `GET /api/v1/runs/{id}`, `POST /api/v1/runs/{id}/cancel`,
  `GET /api/v1/runs/{id}/events`, plus the existing status and health probes. The CLI
  gained `jarvis ask <text>`, `jarvis runs show|events|cancel`, and it **was run**: a
  clean profile, a real `jarvisd`, `jarvis ask "what is the deadline"` printed the
  answer and exited 0, and `jarvis runs show` on an unknown run printed
  `resource.not_found` with advice that matched the status.
  Five properties are structural. **The state mapping is total and lives at the
  boundary** — `wire_state` collapses twelve domain states onto the contract's seven,
  which is what `BRN-005` deferred here because the architecture forbids domain states
  doubling as UI strings; a test asserts it is both total and coarser, and that the three
  terminal states stay one-to-one. (**`BRN-035` corrected this paragraph's implication:** "asserts it
  is total and coarser" was accurate but read as though the *values* were checked, and they
  were not — the covering test compared the count of distinct wire states, so renaming one arm
  passed. The seven names now live in `jarvis_protocol::run::run_state` and a contract test
  compares the set against the document in both directions.) **The authenticated identity is an
  extractor** —
  `AuthenticatedClient` is a `FromRequestParts` implementation, so a handler that lacks a
  credential cannot run, which makes "this route requires authentication" a property of
  its signature rather than a middleware promise in another file, and a test asserts all
  four run routes refuse an unauthenticated caller. **The scope is resolved
  server-side** from the authenticated client, never from a body field, so a caller
  cannot address another workspace's run by naming it. **Idempotency is atomic with the
  mutation** — `create_run_idempotent` writes the run, its opening event, and the key
  record in one transaction, because the contract requires "acknowledged mutation state
  and idempotency records are committed atomically", and a separate claim-then-create
  leaves exactly the window that rule closes. **A reused key with different input is a
  conflict** rather than a second run.
  Six defects were found, five by running rather than reading.
  1. **A cancelled run never reached a terminal state.** A run cancelled before its
     first step had no legal exit from `Received`, so a client polling it waited
     forever for an abandoned answer. Fixed by adding `Received -> Cancelled` to the
     diagram and the machine — the **third** time an edge absent from that diagram
     turned out to be a defect in the diagram.
  2. **A replayed create left an orphan conversation.** The first implementation
     created the conversation *before* checking the key, so a replay reported a
     conflict instead of replaying. The check now precedes any preparation, and a
     concurrent replay discards the conversation it did not need.
  3. **The port event name drifted from the contract.** The delta event was declared
     `run.output_text_delta` while the contract's example says `run.output_text.delta`.
     A cross-check test asserting the port constant equals the wire constant caught it
     on its first run.
  4. **A claim naming an absent run reported the wrong reason.** The foreign key refused
     it, but as the same `Conflict` a concurrent claim produces, so a caller was told
     "someone else used this key" when the truth was "there is no such run". The run is
     now checked first, and `NotFound` is the answer.
  5. **The CLI printed the transport code for a daemon refusal**, so an operator saw
     `jarvis.daemon_rejected` and could not tell an unknown run from a rejected
     credential. It now prints the daemon's own code, and the advice follows the status
     because blaming credentials for a `404` sends an operator to inspect the wrong
     thing.
  6. **`ModelRef`'s owner-name types had no way to build a constant**, so composing the
     daemon's provider needed an `expect` on the startup path — denied by the lint
     policy and wrong regardless. `from_literal` returns `None` instead, so a mistyped
     literal degrades to a provider that serves no model, which reports
     `run.no_model_served` per run, rather than panicking before the daemon can serve.
  The daemon now composes the **deterministic scripted provider**, which is what the
  accepted [first vertical slice](docs/planning/first-vertical-slice.md) names as this
  slice's model source; its reply is a fixed acknowledgement rather than an echo, so the
  deterministic path cannot be mistaken for a real answer and prompt content cannot
  reach a public event payload. `BRN-003` replaces that composition rather than sitting
  beside it. 656 workspace tests (application 86, infrastructure 369, CLI 20). `fmt`,
  `clippy -D warnings`, `node scripts/validate-docs.mjs`, and its fail-closed tests are
  clean. **Not done, and this is why this TODO is `~`:**
  the events endpoint **now follows the run live** — the retained events are replayed from
  sequence 1 and then each new event is pushed as it is published, with **keepalive comments while
  it waits** — and the **CLI now follows it on one connection**, so this paragraph's original claim no
  longer holds in either half.
  **The CLI's streaming renderer is now implemented, and it closed a correctness bug rather than only
  a missing feature.** `follow_run` used to poll: it reconnected every 50 ms with `Last-Event-ID` and
  printed everything the daemon had retained so far, which was the only way to follow a run while the
  endpoint delivered a bounded replay and closed. That was never equivalent to streaming — the answer
  arrived in bursts at whatever the poll interval was rather than as the model produced it — and once
  the endpoint became live it was **also** wrong in a second way, because `runs events` still read the
  body with the buffered helper: a buffered read of a live follow blocks until the run *ends*, and
  caps what it keeps at `MAX_RESPONSE_BYTES`, so a long run's later events were silently **truncated**
  and nothing said so. Both commands now stream.
  - **The client's own streaming path is the piece that made it possible.** `client::stream_response`
    reads the head under a bound, then delivers decoded pieces as they arrive, with the same
    header-decided framing detection and the same refusal to invent content as the buffered path —
    because a stream is where a framing bug is *most* likely, not least. The decode state was extracted
    into `BodyDecoder` once clippy's `too_many_lines` bound was hit, and the extraction is the
    improvement rather than a workaround: the decode is now a value, so its rules are reviewable
    without a socket.
  - **The CLI's SSE parser became incremental, and that is load-bearing rather than tidy.** A streamed
    body arrives in pieces that respect no framing boundary, so a piece can end mid-`data:` line —
    which is normal for any frame larger than the socket buffer. The parser therefore accumulates and
    is asserted against **every split point** of a frame, not one hand-picked fixture; a mutation that
    treats each piece as a whole body fails three tests and names the split. CRLF framing is handled,
    so a `\r` cannot end up inside the last field and turn a healthy frame into a malformed one, and a
    multi-line payload is joined rather than clobbered.
  - **A dropped connection is now resumed rather than abandoned.** One connection lives as long as the
    run does, which exposes it to a failure the 50 ms poll never had — a daemon restart or a socket
    reset would end a follow that had already printed half an answer. The last delivered event id is
    kept and used with `Last-Event-ID`, which the contract makes exact ("resumes strictly after that
    event"), so a resume cannot duplicate output; the attempts are bounded so a daemon restarting in a
    loop fails with a message rather than hanging. A transport failure with **no** position to resume
    from is not retried, because the retry would be the same failure again.
  - **Verified**: `jarvis ask` against Ollama printed the model's multi-line answer in 2.7 s with exit
    0; `runs events` streamed delta and terminal frames in 35 ms and terminated on its own. 1133
    workspace tests (+4, all in the CLI's parser), `fmt`, `clippy -D warnings`, and both doc gates
    clean.
  **What is still not done** in the client half: the run list is absent, and the E2E step's
  abrupt-restart and disconnect cases belong to `BRN-008`.
  **Two of the three items this paragraph used to list were stale, and both are now corrected rather
  than re-asserted.** (1) *"`Idempotency-Key` is scoped per client rather than per
  principal-and-credential"* — **false**: migration `000007_idempotency_principal_scope.sql` rebuilt the
  table on `(workspace_id, principal_id, operation, idempotency_key)`, and
  `lookup_idempotency(workspace, principal, operation, key)` plus a cross-principal test confirm it. The
  paragraph was written before that migration landed and never revised. (2) *"the `jarvis ask`
  conversation-continuation option is absent"* — **true, and now done**: `jarvis ask --conversation <id>`
  sends `CreateRunRequest::conversation_id`, which the daemon has accepted since the create route existed
  while the CLI sent `null` unconditionally, so every `jarvis ask` began a fresh conversation and the
  model received no history. The body is built by an extracted `ask_body(text, conversation)` and
  asserted in two CLI tests, **falsified** by restoring the unconditional `null` (the failure prints the
  body with `"conversation_id":null` and the named conversation dropped). The defect is worth naming
  because **no test could see it**: the request was valid, the run succeeded, and the only symptom was an
  answer ignoring what had been said before — which reads as a model problem rather than a dropped field.
  - **A stale "not done" list is its own hazard.** This paragraph told a reader two things were missing
    when one was already built, which wastes the work of whoever trusts it — the same class as a doc
    claiming a capability that does not exist, in the opposite direction. Both claims were checked by
    reading the migration and the repository signature rather than by re-reading the prose.
  **The golden-fixture half of contract test 12 is now done, and it found two defects.**  `jarvis-protocol`'s `run_contract_tests` and `jarvis-domain`'s
  `model/stream_contract_tests` **read each contract document's own JSON examples** and
  assert the types accept them, rather than checking in copies of them. A copy can drift
  from the document it claims to represent while the test passes, which is worse than no
  test because it produces false confidence; reading the document makes the two
  inseparable. The extraction is what finds a gap — a hand-written fixture list
  reproduces the very mistake round 7 recorded, where `BRN-001`'s evidence listed the
  *tests it wrote* rather than the *contract fields it covered*. **It caught two real
  drifts on its first run:** (1) `ModelStreamEventKind` serialized its `type` tag as
  `snake_case` (`output_text_delta`) while the contract **and the same type's own
  `type_name()`** used the dotted `output.text.delta` — two spellings of one event type,
  which would only have appeared at the far boundary; (2) the enum was `#[serde(flatten)]`ed
  onto the envelope, so an event's `item_id` and `delta` landed at the top level, while
  `model-stream.md`, `event-envelope.md`, and `local-control-api.md` all nest a variant's
  fields under `payload`. Both are fixed, and a test now asserts the **serialized** shape
  as well as the parse, since only both together make the type and the document
  interchangeable. The contract's `route_requirements` example was also stale: it showed
  `tools`/`structured_output`/`local_only` booleans where the accepted design uses the
  **same `Capability` vocabulary the inventory attests** and the `Locality` the data policy
  resolves — a parallel set of flags would let a requirement exist that no capability key
  could satisfy, and both sides would still compile, so the example was corrected to the
  design `BRN-001` had already justified. **Still not done:** the generated `OpenAPI`
  document, and the SSE fixtures beyond framing (a full frame's `data` is asserted by the
  existing envelope test, not by a golden file).
  **The live SSE follow is now implemented, and the reason it was not is the interesting part.**
  The endpoint used to deliver the retained events and close, so a client had to reconnect with
  `Last-Event-ID` until a terminal event arrived. The contract was always the live behaviour
  ("Initial connection replays retained events from sequence 1, then follows live events"), so
  this was a conformance defect rather than a scope decision — but the recorded justification
  for it was **wrong on both of its two claims, and checking them is what unblocked the work**:
  (a) it named "`axum`'s `sse` feature" as missing from the reviewed dependency set, and axum
  0.8.9 **has no `sse` feature at all** — `axum::response::sse` is unconditional; (b) it would
  have required adding a stream crate, and the two candidates were **already in `Cargo.lock`**
  (`futures-core` 0.3.34 as a leaf, `tokio-stream` 0.1.19 through `sqlx-core`). Naming
  `futures-core` directly adds **no new crate and no version change** — the lockfile diff is
  one line inside `jarvis-infrastructure`'s own dependency list — so the change reduced to a
  routine dependency-name addition plus a ledger row, which is the process `AGENTS.md`
  prescribes rather than an obstacle to it. **A "blocked on dependency review" note is a claim
  about the dependency graph, and it has to be checked against the graph, not remembered.**
  - **The design avoids the hard half rather than solving it.** The stated worry was that a
    hand-written `Stream` would be "an unreviewed async state machine on the security-relevant
    path", and that is a real hazard: a `poll` that owns a future borrowing the state it must
    also reach needs unsafe code or a self-referential type. The response body is therefore a
    **bounded channel fed by an ordinary `async` follow task**, with a thin `Stream` impl that
    delegates to `mpsc::Receiver::poll_recv` — exactly the shape
    `jarvis_application::model::AdapterStream` already uses for a provider's frames. No unsafe,
    no macro, and the follow loop is readable `await` code.
  - **Four properties, and each one is a decision.** (1) **The durable store is the source of
    truth**: the follower's position is a *sequence* and every read is `load_events(from)`, so a
    live stream and a replayed one cannot disagree about content or order. (2) **A wake-up
    carries no payload** — it is permission to read again, which is what makes a bounded buffer
    safe: a slow follower loses *wake-ups*, not events, so "a slow consumer can replay from its
    last delivered event" holds without a special case. (3) **The stream ends on the durable
    terminal state, not on a notification**, so a last notification that never arrives cannot
    leave a client waiting after its run is over. (4) **The refusals are decided before the body
    streams** — `400` for an `Accept` mismatch and `409` for an unavailable resume position —
    because once the response has begun a status code can no longer express a decision.
  - **The test needed a new fixture, and that is the part worth remembering.** With a provider
    that finishes immediately, every event is already retained by the time a client connects, so
    a replay-and-close handler and a live one produce the **identical body** — which is why the
    pre-existing stream test passed for as long as the feature was missing, and why it could not
    have caught this. The new test drives a **gated** provider that publishes one delta and then
    blocks on a `Notify`, so "the run is live and unfinished" is an observable state; the
    assertion is that the response body has **not** completed while the run is live. **Verified
    by mutation**: restoring the replay-and-close behaviour fails it, and the failure message
    names the defect rather than a timeout. Its counterpart covers the other termination rule —
    a follow of an already-finished run replays and closes instead of waiting for a notification
    nothing will send. 1113 workspace tests (+2); `fmt`, `clippy -D warnings`, and both doc gates
    clean, including `--changed-file` over `Cargo.toml`, `Cargo.lock`, and the evidence files.
  - **A real defect existed only in the live path, and the E2E journey found it — not a unit test.**
    The follower's read position was initialised from the *resume* sequence instead of from the page
    it had just rendered, so the next read returned the same page: every replayed event was delivered
    **twice**, the body restarted at sequence 1 in the middle, and the run appeared to publish **two
    terminal events**. This is a shape a replay-and-close handler *cannot* exhibit, because it performs
    exactly one read and never advances past it — so the defect was created by the new code and was
    invisible to every existing test. `disconnect-journey.mjs` caught it through its own existing
    assertions (terminal count, contiguous sequences) with no change to the harness, which is the
    payoff of writing an E2E check against a *property* rather than against one implementation's
    output. A regression test now drives a whole live stream and asserts the sequence list is
    `1..n` exactly once; **falsified** by restoring the wrong position, which fails both live tests.
  - **The regression test was itself vacuous at first, and writing a second test is what exposed it.**
    Its sequence extraction used `strip_prefix("data: {")`, which removes the opening brace — so the
    remainder never parsed as JSON, the filter produced an **empty** vector, and `assert_eq!` compared
    two empty ranges and passed. A test that cannot fail. The second live test's explicit
    non-empty precondition is what surfaced it, and the fix is a shared `sequence_numbers` helper that
    parses the whole value, so the next assertion cannot be wrong in a different way.
  - **Also found by reading the contract while implementing this:** the cancelled terminal event
    still carries no payload, and the contract's own Cancellation section names the reason as
    caller-supplied text needing escaping. A failed event's payload is safe because every value
    in it comes from a closed set; a cancellation reason is free text, so **a public payload
    cannot simply echo it** and the fix is a redaction/omission decision rather than a
    serialization one. Left as a named gap rather than half-done, and it belongs with the
    remaining stream work below.
  - **Keepalives are now sent, which closed a second producer-less frame.** The contract requires
    them ("Keepalives are SSE comments and do not consume sequence numbers") and
    `jarvis_protocol::run::keepalive_frame` already existed **with a test asserting its shape and no
    caller** — the product could describe a comment it never sent, which is the same shape as the
    `LiveRunEvents`-with-no-implementer problem and the reason a "helper nothing calls" is treated
    here as an unimplemented feature. The follow loop now runs a `select` between a wake-up and an
    interval tick, emitting one comment per interval **only while it is waiting** — the state the
    comment exists for, since a run that is *thinking* has nothing to send and a silent connection is
    dropped by whatever sits between the daemon and the client. Three properties are deliberate.
    (1) **It is a comment**: no `id:`, so no sequence number is consumed, and the test asserts the
    *contiguity of the event sequences* across a stream containing comments — the same rule stated
    the way a client experiences it, because a keepalive modelled as a synthetic event would send a
    resuming client to a position that never existed. (2) **The timer lives at the loop, not at each
    wake-up**, so the interval measures between keepalives rather than being reset by every event; and
    **the first immediate tick is consumed**, because a live stream normally opens with a *frame* and
    an unconsumed first tick made it open with a comment telling a client that nothing had happened
    before it had been told the run existed. (3) **Missed ticks are not repaid as a burst**
    (`MissedTickBehavior::Delay`), since several comments at once is traffic with no purpose.
    **Falsified** by making a tick emit nothing: the keepalive test fails. The interval is injectable
    (`ApiState::with_keepalive_interval`, production `DEFAULT_KEEPALIVE_INTERVAL = 15s`) because a
    keepalive is only observable by *waiting*, so a production interval would make its test take as
    long as the interval. 1116 workspace tests (+2); `fmt`, `clippy -D warnings`, and both doc gates
    clean. **Still not done at this point:** an explicit slow-consumer disconnect with `stream.overrun`
    (a slow client was backpressured rather than told it fell behind), and the CLI still followed by
    reconnecting. **The overrun half is closed by the bullet below; the CLI half was closed by the
    single-connection follow above.**
  - **The slow-consumer disconnect is now implemented, and the contract sentence was the spec.** "Per-client
    buffers are bounded. A slow consumer is disconnected; it can replay from its last delivered event
    while retention permits." The buffer *was* bounded — a full follow channel parks the follow task —
    but a **parked** follower was told nothing, and a client that has stopped reading cannot distinguish a
    silent daemon from a run with nothing to say. The bound existed; the disconnect did not. Two pieces
    were needed, and the split is the finding rather than the implementation:
    1. **A bound on one hand-off, measured on delivery.** A follower that is keeping up never waits on a
       send, so `DEFAULT_STREAM_OVERRUN_TIMEOUT` (10s) cannot fire for one; a follower that has stopped
       reading waits for ever. This is what makes the buffer bound *observable* rather than merely real.
    2. **A signal that travels BESIDE the channel, not through it.** The one moment an overrun exists is
       the moment the channel is full — a follower that were reading would never trigger the bound — so a
       signal sent through it would be blocked behind the congestion it is describing, and `try_send`
       would simply fail. It is recorded in a `watch` beside the channel and delivered by the response
       body as its **last** item, after everything the daemon did manage to hand over. Delivering it last
       is also what makes the client's instruction actionable: "resume from the last event id you saw",
       and the last frame it read is exactly that.
  - **Nothing is dropped and nothing is skipped, and that is why the bound is on time rather than on a
    position.** The events are durable, so a client that is merely slow loses nothing when it reconnects —
    which is the property that makes "disconnect the slow consumer" safe at all. Bounding by position
    instead (skipping what could not be delivered) would leave a permanent gap, the one outcome the
    contract forbids outright.
  - **The signal is a real event and not a comment, the mirror of the keepalive's reasoning.** A comment is
    for a client that is *waiting*; this is for a client that must *act*. It carries no `id:` and consumes
    no sequence number, so resuming on it returns the client to the last genuine position rather than one
    that never existed — and it is emitted as the stream's last frame, not beside a terminal, because a
    terminal means the run ended and this means the *connection* did.
  - **A defect I introduced and the compiler could not see.** `watch::Receiver::borrow_and_update` marks a
    value *seen*; it does not consume it. A `Stream` is polled again after every `Ready`, so the body would
    have emitted the overrun frame **for ever** instead of ending. One-shot-ness is tracked explicitly
    (`overrun_delivered`), because the contract makes the frame one-shot and nothing in the watch channel
    does.
  - **Two attempts at the test were wrong for reasons worth keeping.** The first flooded 64 frames — fewer
    bytes than a loopback socket buffer, so the kernel absorbed the whole response and the run completed
    normally, with a `200` and no signal; the flood must now clear an asserted byte floor, because a test
    whose trigger fits inside the buffer it is not measuring passes against the defect. The second drove a
    real `TcpStream` that never read, which *should* work but adds the OS send and receive buffers between
    the daemon and the bound — a layer that cannot be sized portably. The test now polls the response body
    through the router and simply does not collect it for several bounds, which leaves the bound it is
    about and nothing else. It also asserts the delivered frames are **contiguous from sequence 1**, since
    a skipped event would be lost rather than deferred.
  - **The CLI classifies the signal rather than recognising it**, and the classification is a function over
    the frame (`follow_step`) because the interesting property is what a frame is *not*: an `if` inside a
    streaming callback can only be tested with a daemon on the other end. Asserted over **every** event
    type the protocol defines and in both directions — the signal resumes, and the three real terminals
    still end the follow. That second half is not decoration: the arms are adjacent, so an edit that
    widened the signal's arm would swallow a terminal and leave the client following a finished run.
    **Falsified** by making the signal a success, which fails the assertion naming it.
  - **The contract's minimum-code table gained its first `n/a` status row**, and the row is the finding:
    `stream.overrun` is delivered *inside* a stream after the `200` has begun, so there is no status left
    to carry it. Both table parsers now share one row predicate that accepts `n/a`, rather than the shape
    check being written twice — two copies is how the code comparison and the flag comparison come to
    disagree about which rows they see.
  - **The retry decision is now a function over values rather than an `if` chain in a live loop.** The
    clause that is wrong is the clause nobody tests, and this loop is the worst place for that: it is
    driven by a connection to a real daemon, so reproducing "a transport failure on attempt 2 with no
    position" needs a daemon failing in that exact way. `follow_after` takes the outcome, the overrun
    flag, the attempt number, and whether a position exists, and returns *resume* or a typed report —
    so every combination is enumerated by a test. Writing that test immediately found a real
    conflation: an overrun with **no position** and an overrun that **exhausted its budget** were the
    same branch, so a client that had retried three times was told there was no position to resume
    from. They are different facts and now have different outcomes
    (`OverranBeforeAnyEvent` vs `OverrunBudgetExhausted`).
  - 1144 workspace tests (+8: 4 in `jarvis-protocol`, 1 in `jarvis-infrastructure`, 3 in `jarvis-cli`).
    All five gates + `cargo doc` green. **DO NOT COMMIT.**
  - **Two defects found by sweeping this round's own work, both of the same class this project has now
    seen six times — "a value that exists but is enforced nowhere".**
    1. **A bound I had written one round earlier and never enforced.**
       `MAX_STREAM_BYTES_PER_DELIVERY` was declared in `jarvis-protocol` with a doc comment calling it
       "the largest number of bytes the daemon will spend handing one event to one follower", and its
       reference count was **exactly one — its own definition**. Nothing read it, no test named it, and
       it had no enforcement point; the honest fix was deletion, because the byte bound does not exist
       and the *time* bound is what implements the disconnect. **The trap is that a declared bound
       reads as coverage**: had it not been deleted, the next reader would have concluded that frames
       are size-bounded, which is the belief the constant's own comment states. Sweep the constants you
       *added* in the previous round, not only the ones an earlier round left behind.
    2. **A test that could not fail, in the same crate.** `every_output_chunk_is_published_before_the_run_completes`
       filtered `event.event_type == "run.output_text_delta"` — an **underscore** where the event this
       daemon publishes is `run.output_text.delta`, which a *different* assertion **in the same file**
       spells correctly. So the filter matched nothing, the loop body never executed, and the ordering
       property the test is named for was checked **zero** times. Nothing failed, because a `for` over
       an empty selection is still a passing `for`. Fixed in the two ways that each half a trap: the
       name now comes from `OUTPUT_TEXT_DELTA_EVENT` (a rename becomes a compile error rather than a
       silently empty filter), and a **non-empty precondition** is asserted, which is what turns
       "checked nothing" into "failed". **Falsified** by restoring the underscore, which fails with the
       event dump showing the two real deltas at sequences 5 and 6. This is the same shape as the
       resume test whose trigger sat inside the page it searched.
    - The generalisable rule, stated because it has now caught two different bugs: **when a filter can
      produce nothing, assert there is something before asserting a property of the contents.** A
      property over ∅ is vacuously true, so an empty selection and a correct one are indistinguishable
      from the assertion alone. A `for`-over-filter sweep found no other instance of this shape in the
      workspace, and the remaining `filter`s each have a non-empty precondition of their own.
  - 1144 workspace tests (unchanged: the corrected test replaces its own body). All five gates +
    `cargo doc` green. **DO NOT COMMIT.**
  - **A raw capture of a real stream closed `OC-C004`'s open half and found a field the documented
    schema does not have.** The `[DONE]` question had been recorded as UNVERIFIED for rounds because
    the cited OpenAI page does not mention the sentinel, and the fix the evidence note named was to
    *capture one stream from the operator-configured endpoint* — which is what an Ollama instance on
    loopback makes possible without a credential.
    - **`[DONE]` exists.** The capture ends `data: [DONE]\n\n`, so `OC-C004`'s `[DONE]` half moves from
      `UNVERIFIED` to `OBSERVED`. It also carries `finish_reason: "stop"` **before** the sentinel, so
      the adapter's deliberate refusal to *depend* on `[DONE]` is vindicated for this server rather
      than merely defensible.
    - **`delta.reasoning` was on most of the frames and is absent from the documented field set.** The
      capture's shape is five frames of model-internal thinking with an empty `content`, then the
      answer, then the terminal, then a usage chunk, then `[DONE]`. The adapter already discarded that
      text — by the accident of reading only `content`, `tool_calls`, and `refusal`, which is exactly
      the defect: **a field no code names is indistinguishable from a field the translator forgot, and
      the two have opposite remedies.** One is a security decision to preserve, the other a bug.
    - Fix: `reasoning_chunks_ignored` counts it and it is **never** translated, with the code stating
      why it is not mapped onto `reasoning.summary.delta` — that event carries a *user-visible summary*
      the contract permits, while this is raw thinking the memory rules forbid persisting, so mapping
      one onto the other would be the violation wearing a legitimate event name.
    - **Falsified** by emptying the increment, which fails both new tests. One of them is built from the
      **captured frames themselves**, so the fixture and the wire cannot drift.
    - **The generalisable rule, and this note has now taught it three times:** a documented field set is
      a claim about a *page*, not about an endpoint. Every mapping row in the evidence note was derived
      from the documentation, and two of the three things a real server forced are things the
      documentation does not contain.
  - **Two accessors whose doc comment described a caller that does not exist.** `delta_count()` claimed
    "Used by the adapter to report the measurement `BRN-011` aggregates" — and nothing outside the
    module reads it, which a reference count showed immediately. `late_deltas_ignored()` and the new
    `reasoning_chunks_ignored()` are in the same position. They are now `#[cfg(test)]` with an honest
    doc, because the alternatives are worse: leaving them `pub` with a false claim is the defect this
    project keeps finding, and wiring a counter into an observability surface is a feature this round
    is not. **The measurement `BRN-011` aggregates is real and is not this counter** — `model_calls`
    records `output_delta_count` from the events the controller consumed, which is the value a re-read
    run still has.
  - 1146 workspace tests (+2). All five gates + `cargo doc` green. **DO NOT COMMIT.**
- [ ] `BRN-008` Implement cancellation, timeout, disconnect, fallback, and daemon
  restart behavior. This TODO owns the run controller and the repositories' test
  doubles: `jarvis_application::run_controller` drives one durable run from
  `Received` to a terminal state, and `jarvis_application::testing` supplies the
  in-memory repository port implementations that let a controller test assert what
  was **persisted** rather than which mock was called.
  Evidence: `RunController::execute` joins the four pieces the earlier slices built —
  it advances the state machine through `RepositoryTransition`, performs one model
  turn through `ModelProvider`, reads the transcript through `ConversationRepository`,
  and records the attempt through `ModelCallRepository`. That a plain question now
  reaches `Completed` is the direct product consequence of the `BRN-005` edge fix: the
  path is `Received -> ContextBuilding -> Planning -> AwaitingModel -> Responding ->
  Completed`, and the test asserts five activity events were published, one per
  transition, with the next sequence at six — because the storage architecture's rule
  is that a state change and its event commit together, so a state that moved without
  an event would be a silent gap in the audit trail. **Three defects were found only
  by driving the real path.** First, the model roster was checked *after*
  `AwaitingModel` was entered, so a provider serving no model left the run **waiting
  for a call that could never be made, in a state with no legal way out** — the check
  now runs from `Planning` and the run ends `Failed` with `run.no_model_served`;
  `selected_model` returning a fabricated fallback would have hidden a
  misconfiguration and then failed at the provider with a confusing error, which is
  why an empty roster is refused by name. Second, a provider that refused to open a
  stream left its `model_calls` row `Pending`, so a later reconciliation pass could not
  tell whether a call was outstanding, and a frame refused mid-stream left it open the
  same way — every path that ends a model call now records its terminal outcome.
  Third, `build_request` took `&self` while reading no port and holding no state, so
  it is now a free function; the controller's helpers group their arguments
  (`RunRef`, `Step`, `ModelTurn`) rather than passing seven or eight scalars, which is
  also what removes the possibility of a workspace/run-id transposition at a call
  site. Four properties are structural rather than asserted. **A cancelled call ends
  `Cancelled`, not `Failed`** — `ControllerError::terminal_state` maps a cancellation
  to `Cancelled` and every other outcome to `Failed`, so one decision covers every
  path. **A cancellation that arrived before any work leaves the run untouched** in
  `Received` with no event, because moving a run to `Cancelled` for a request that
  never started records work that did not happen. **A stream with no terminal event
  fails the run**, using the domain's `StreamOutcome::Interrupted` rather than a
  boolean completion, because the contract is explicit that a stream ending without a
  terminal is not success. And **a tool intent is refused with a typed, terminal
  `run.tools_not_implemented`** rather than answered with a fabricated observation:
  the fabric is Milestone 3, `ControllerError::is_unimplemented` distinguishes the
  known gap from a fault so a caller reports "not built yet" differently from
  "something went wrong", and the controller performs exactly **one** model turn
  because a loop that cannot iterate a second time would be a claim with no behaviour
  behind it. The in-memory double deliberately **does not reimplement the transition
  table**: it delegates edge legality, version ordering, and terminal absorption to
  `RunLifecycle`, exactly as the SQLite adapter does, and adds only the storage
  semantics (transaction-and-event fusion, the uniqueness constraints, scope as a
  query predicate), because a double that copied the rules would be a second
  implementation that could disagree with the first and a test passing against it
  would prove only self-consistency. 67 application tests (46 -> 67; 593
  workspace-wide). `fmt`, `clippy -D warnings`, and `node scripts/validate-docs.mjs`
  are clean. **Not done**: this is one model turn with no tool execution and no repeat
  — steps 3, 4, and 5 of the architecture's native runtime list are the tool fabric
  (`TLS-001` through `TLS-012`); there is no turn/token/cost/time budget enforcement,
  no retry or fallback on a retryable provider error, no `Waiting` state entry because
  nothing suspends and resumes yet, the `agent_steps` table still has no port or
  adapter, no timeout or daemon-restart behavior is implemented, no endpoint or CLI
  path reaches the controller, and the run's state is not yet mapped onto the
  client-visible set — that projection remains `BRN-007`'s, since the architecture
  forbids domain states doubling as UI strings. `BRN-009`'s deterministic
  orchestration tests and the gated provider smoke test are still outstanding.
  **Daemon-restart behavior is now implemented.** The local control API's rule — "on
  restart, terminal runs remain terminal; nonterminal runs are recovered to an explicit
  resumable or failed state" — is `jarvis_domain::run::recovery` (classification) plus
  `jarvis_application::recovery::reconcile` (the pass), called from `jarvisd` **before**
  discovery is published and before readiness is marked, because the contract requires
  readiness to stay false until recovery classification completes. **Two missing
  diagram edges had to be added before this was expressible**: `AwaitingApproval -->
  Failed` and `Observing --> Failed` were the only non-terminal states from which
  `Failed` was unreachable, so a run interrupted while awaiting a decision or while
  folding a tool result had **no legal terminal exit** — the fourth time in this project
  that a missing edge, rather than a missing design, made a real outcome unexpressible.
  The classification is the domain's because which states an interrupted run may be
  found in is a statement about the machine, and `classify` is an exhaustive match so a
  new state fails to compile rather than silently defaulting to "leave it alone" — the
  one default that leaves a run non-terminal forever. `TransitionActor::Supervisor`
  records that the daemon ended the run, not the run. The new read
  (`RunRepository::incomplete_runs`) is **deliberately unscoped** and returns each run's
  workspace, because recovery is a whole-profile startup concern and scoping it to one
  workspace would leave every other workspace's interrupted runs non-terminal with no
  symptom. The version read is the version written, so a run that completed between the
  read and the write is refused (`storage.transition_refused`) rather than having a real
  outcome replaced by a failure — which is what makes the pass safe against a live
  database. Each run is written independently, so one unsettleable run cannot leave the
  rest non-terminal, and the report distinguishes a complete pass from a partial one so
  a caller cannot read "nothing to do" out of a pass that failed to read. **Nothing is
  resumed**: both classifications land on `Failed`, because resuming means re-running a
  model call and nothing knows what the interrupted call produced; a parked run is still
  classified separately (`was_resumable`, `parked_in` in the payload) so the distinction
  survives. The startup order is proved end-to-end on a durable database file across
  **three real daemon starts** — the first leaves a run mid-flight, the second must
  settle it and report the count, the third must find nothing — and that test was
  confirmed to **fail** when the pass is made to find nothing, so it falsifies the
  feature rather than describing it. 11 application tests plus 4 adapter tests against a
  real migrated SQLite database. **Not claimed:** no run is resumed or re-driven, and no
  dependency a parked run was waiting on is re-evaluated.
  **Time budgets are now enforced.** `jarvis_domain::run::budget` holds `RunBudget` — a
  deadline, a step timeout, and token/cost ceilings — and the pure arithmetic that decides
  whether one is spent, so "this run is out of time" has exactly one definition rather than
  a comparison repeated at each call site. Two facts were wrong before this: a run had **no
  deadline at all** (`agent_runs.deadline_at` and `budget_json` were schema columns with no
  port able to populate them, so they were always NULL), and the controller sent
  `limits.deadline: null` to every provider, so a provider honouring the contract's own
  `limits.deadline` had nothing to honour and one that hung held the run open indefinitely.
  The controller now bounds **both** awaits — the provider `open` and each frame wait — with
  the tighter of the remaining deadline and the step timeout, re-derived per frame so a
  stream that consumed most of its time cannot exceed the run's own limit. A run whose
  deadline had already passed is failed before a provider is contacted, so no model call is
  recorded and nothing is billed. `ProviderError::Timeout` is a new variant because a
  deadline JARVIS set is a different fact from an unreachable provider — the operator looks
  at the budget rather than the provider's status page — and it is deliberately **not**
  retryable, because an expired deadline leaves no budget to retry inside. Every run created
  through the service now carries a bounded default budget, since an unset budget is not
  neutral: it means no deadline at all. The bounds were proven to be what makes the tests
  pass by removing them, at which point the three hang tests **hang for 60+ seconds** rather
  than fail. 16 domain tests, 8 controller tests, 2 service tests, and 4 adapter
  round-trip tests. **Token and cost ceilings are now enforced too.** The provider's
  reported usage is captured from **either** arrival path — its own `usage.updated` frame
  or the terminal's block, since the contract says a usage frame may arrive before, with,
  or after completion — and the last one wins, because a later frame is a revision and
  taking the first would under-count a run that then slips past a ceiling it breached. The
  run's ceilings are checked against that usage before the run is allowed to complete, and
  a breach **fails the run and discards the answer**: a run that breached a ceiling and
  still returned its output would make the ceiling advisory, and a caller could not tell an
  enforced limit from a cosmetic one. The comparison is strictly greater-than, so a call
  that used exactly its ceiling is inside the budget — the opposite boundary convention
  from the deadline, and consistent with it, because a deadline at `T` does not permit work
  at `T` (that work finishes after `T`) while a token ceiling of 2048 permits producing
  token 2048. A ceiling with **no reported usage cannot breach**, because failing every run
  against a provider that omits usage is a false failure rather than a safety property;
  `budget_is_verifiable` states the gap instead of hiding it, since an unchecked ceiling is
  the state most easily mistaken for an enforced one. The usage and the cost lifted from it
  are written in one call, so a ceiling check and a cost query cannot read different amounts
  for the same call. Falsified by making the comparison never breach, at which point all
  three enforcement tests fail. **The summed figure now exists** (`BRN-058`): a run's calls
  are read back through `ModelCallRepository::load_run_calls`, `Usage::summed` folds them,
  and `GET /api/v1/runs/{id}` serves the total — so "what did this run consume" is answerable
  rather than only "what did the one live call consume". The **enforcement** path still
  compares one call's report, which is now a correctness gap with a producer available
  rather than a missing read; **not done:** the ceiling is not yet judged against the sum,
  there is no turn budget, no byte or concurrency budget, no retry budget, and the
  disconnect case remains open — so `ACC-073` is closer but not closed.
  **Retry is now implemented, and it found a defect.** `jarvis_domain::run::retry` holds
  `RetryPolicy` and the pure decision that applies it; the controller honours it, so a
  transient failure **before the provider accepted the call** closes the attempt and leaves
  the run **live** so another attempt can be made. The retry-chain storage (`logical_call_id`
  plus `attempt`) was built by `BRN-004` and had **never been used** — every attempt was
  attempt 1, and a transient failure ended the run. The contract's safety boundary is
  enforced by construction rather than by intention: `FailureSite` is a **required input** to
  the decision, because the same error must decide differently on either side of acceptance,
  and the ambiguity rule is checked **first** so no other rule can reach a retry for an
  ambiguous request. Three further rules: a retry must fit the deadline (a backoff that
  outlives it is a delay followed by the same failure, and a budget-caused refusal is
  reported as the deadline rather than a provider fault); a policy that does not retry is the
  **default**, because retrying spends a budget the caller never offered; and no retry is
  attempted when consumption ceilings were breached, since the same output would breach
  again. **The defect:** a provider error arriving *mid-stream* was propagated with **no
  transition at all**, so the run was left in `AwaitingModel` — non-terminal, and looking
  exactly like a run about to retry. A client would poll it forever and only a daemon restart
  would settle it. It is the same class as the missing terminal exit `BRN-007` recorded, and
  it was invisible until a test asserted the run's stored state rather than only the returned
  error. Both new behaviours were falsified: disabling the retryable branch fails 5 tests, and
  disabling the ambiguity rule fails 2. 18 domain tests, 9 controller tests, and 1 adapter
  round-trip. **Not done:** **no fallback** — this needs a capability inventory and candidate
  routes (`BRN-003`, `BRN-010`), so the contract's fallback list is not implemented. The retry
  policy **is now settable per request** (`BRN-050`: `CreateRunRequest.retry`, defaulting to a
  bounded three-attempt policy when absent), so the earlier "no typed override" gap is closed;
  the disconnect case remains open.
  **Cancellation and disconnect are now implemented and proved end to end, and the proof
  found four defects.** The disconnect case had been deferred three times because nothing
  exercised it: `jarvis ask` follows a run to its terminal, so a client that *disappears* is
  not something the CLI can express. `tests/e2e/disconnect-journey.mjs` is the first
  executable harness in `tests/e2e/`, speaks the local control API directly with Node's
  standard library against a real `jarvisd` and a fresh `--profile`, and asserts durable
  state read back over the API rather than only status codes. **First, the cancel status
  was derived from a separate pre-read** of the run, so a run that finished between the
  two reads was reported `202` as though cleanup were in flight while the service had
  already found it terminal — two answers for one fact. The status now comes from the
  service's own returned state, and the unit test that had *accepted any terminal state*
  was rewritten, because a test loosened to accommodate a bug encodes it. **Second,
  `ContextBuilding`, `Planning`, and `Observing` had no `Cancelled` edge**, so a run
  cancelled in one of those states had **no legal exit and sat there forever** — the fifth
  instance of the missing-diagram-edge class (the first four were `Planning -> Responding`,
  `AwaitingApproval -> Failed`, `Observing -> Failed`, and the retry path's terminal), and
  found by a live repro rather than by an argument. Every non-terminal state now reaches
  `Cancelled`, and the architecture diagram gained the three edges. **Third, the entry
  cancellation check hard-coded `Received` as the transition origin**, so a cancel arriving
  after the run had advanced was refused as an illegal edge — and the refusal was
  **swallowed by the detached task**, so the cancel was accepted by the API and then
  silently did nothing. The check now loads the run and transitions from its actual state;
  the step boundary is a helper that checks cancellation *after* each advance, and there
  are now checks after the stream drains and before delivery completes, from which
  `complete_run` **discards an answer** rather than storing one for a run the caller
  cancelled. **Fourth, and the one that explains everything else: the storage layer refused
  a contended transition with `database is locked` (`SQLITE_BUSY`) and reported it as a
  generic `Query` fault.** A deferred `BEGIN` takes a *read* lock and upgrades to a write
  lock at the first write; when two transactions both want that upgrade SQLite fails one
  **immediately and without consulting the busy handler**, because waiting would deadlock —
  documented, deliberate behaviour, so the profile's `busy_timeout` provably did not cover
  the one case that mattered. The loser's `UPDATE` failed, the terminal transition was
  refused by a *lock* rather than by a rule, and the run was left permanently
  non-terminal — stuck in `context_building` or `responding`, about one run in three. The
  fix is `BEGIN IMMEDIATE` (`pool.begin_with`, and `&'static str` implements `SqlSafeStr`),
  applied to all four write transactions, so the conflict happens where the busy handler
  *does* apply. This is why a contention condition must not be reported with a transport
  error code: a caller cannot tell "try again" from "your view is stale".
  `concurrent_transitions_on_one_run_never_fail_with_a_storage_fault` is the regression
  test, and it is deliberately two-sided — every result must be a success or a typed
  `VersionConflict`, and at least one write must land, because a test that accepted
  all-conflicts would pass against an implementation that refused everything. It runs
  against a **file** database, because the in-memory fixture is pinned to one connection
  by design and *cannot* produce contention: the check was confirmed to **fail** under
  `BEGIN DEFERRED` with the exact live fault (`reported Query`) and pass under
  `BEGIN IMMEDIATE`, and the journey then passed eight consecutive runs where it had
  previously failed four in eight. The deterministic half of the contract — that a cancel
  arriving *during* delivery must win — is covered by a provider double that cancels from
  inside its own stream (`CancelsMidStream`), so there is no race to lose, and it asserts
  both that the run ends `Cancelled` and that the partial output was **not** stored as an
  assistant message. 2 controller tests, 1 adapter regression test, 1 E2E harness.
  **The prompt is now bounded, which closes the last item on the native-runtime list that
  could be closed without a tool fabric.** `RunBudget::max_context_tokens` plus
  `jarvis_application::context_assembly` mean the controller builds its request from what fit
  the run's context ceiling rather than from every message it read, so the budget's "token"
  dimension covers the input side as well as the output side. An assembly failure or a context
  that dropped the run's own objective leaves the run **terminal** at `ContextBuilding` — the
  sixth instance of the missing-terminal-exit class, and the second found by asserting stored
  state rather than a returned error — because the first version propagated the error with `?`
  and left the run with no legal exit. See `BRN-006` for the three defects this found.
  **Not done:** no fallback, per-request retry policy, or resumption, as above.
- [~] `BRN-009` Add deterministic orchestration tests and gated provider smoke test. **The gated
  provider smoke test is done; the deterministic orchestration tests are substantially covered but
  not as one named suite, which is why this stays `~`.**
  Evidence: **the gated half now runs against a real model.** `tests/e2e/provider-smoke.mjs` starts a
  daemon configured for Ollama on loopback, creates a run through the real control API, and asserts
  the daemon is wired to the configured provider (not the scripted fallback), that the run reaches
  `completed`, that the durable events carry the **model's own text** rather than the scripted
  acknowledgement, and that `jarvis ask` prints that answer and exits 0 — which is Milestone 2's exit
  gate, *"a gated real-provider smoke test streams a response"*, stated as an executable check. Live
  run verified: `jarvis ask "Reply with exactly: JARVIS OLLAMA OK"` printed `JARVIS OLLAMA OK`, exit
  0, against `127.0.0.1:11434`. The harness takes host, port, base path, and model from the
  environment, so pointing it at another compatible server is a configuration rather than a code
  change. **The gate is a reachable endpoint, not a secret**, because the adapter is loopback-only by
  construction and no cloud endpoint is reachable from this build; and a skip is **reported** as
  "nothing was proved" rather than printed as a pass, because a CI log that cannot distinguish the two
  is exactly what a gated test must not emit. **Not this TODO's half, but found while wiring it:**
  the real endpoint required a configurable base path and a model-name mapping (see `BRN-003`), and it
  exposed a chunked-transfer defect in the CLI's own client. **Still outstanding:** the deterministic
  orchestration tests exist and are extensive — cancellation mid-stream, a retryable failure leaving
  the run live, a deadline already passed, each consumption ceiling, a refusal settling the run, and
  the tool-intent refusal — but they are distributed across `run_controller`, `run_service`, and the
  HTTP surface rather than gathered as the contract test 7/8 suites `BRN-009` names, and the
  contract's disconnect/reconnect cases belong to `BRN-008`'s journey.
  - **Contract test 7's sequence-gap case is now implemented, and it was the one part of that sentence
    no code honoured.** The contract requires that clients "never ignore a sequence gap"; the reference
    client satisfied that by **not looking** — it parsed `event_id`, `event` and the payload, and never
    read `sequence` at all, so a gap could not be noticed because the value was never read. That is the
    same failure the rule names, reached by omission instead of by decision, and it is the worse version:
    the client keeps printing across the gap and delivers two halves of an output that were never
    adjacent, as a complete answer.
    - **A gap is fatal rather than resumable, and the reason is that it is unrepairable.** The client
      cannot know which events it missed or whether they are still retained, and its one recovery
      mechanism — `Last-Event-ID` — re-reads from the position it already reached, leaving the gap exactly
      where it was. So the check **outranks the retry decision**, because a gap consulted through the
      retry path would be answered as a dropped connection and reconnected.
    - **Two boundaries keep it from refusing healthy streams.** A resumed stream's first frame is exempt
      (`Last-Event-ID` resumes *strictly after* the named event, so requiring contiguity across a
      reconnect would refuse the client's own recovery path), and a frame with no readable `sequence` is
      not a gap (inventing one from an absent field would refuse a stream the daemon is sending
      correctly). A fresh stream must still open at sequence 1, which is asserted separately.
    - **⚠ The first version of the end-to-end test was vacuous, and the mutation is what said so.**
      It ended the stubbed body after the gapped frames, and disabling the entire gap check **passed**:
      a stream with no terminal event fails the follow as `EndedWithoutTerminal` whether or not the gap
      was noticed, so `assert_ne!(SUCCESS)` held for the wrong reason. The stream now carries a terminal,
      so only a client that notices the gap fails — verified by re-running the same mutant, which then
      failed. **A gap-ignoring client has to be able to succeed for the assertion to be about the gap**,
      which is the same "assert there is something before asserting a property of it" rule that found
      the underscore defect in round 79, applied to a test's *expectation* rather than to its filter.
    - Checked against a **real socket**, not only as a pure function: a stub listener on an ephemeral
      loopback port serves a stream, so the wiring is exercised rather than assumed. A correct function
      nothing calls is precisely the shape of the original defect.
    - **Still outstanding for `BRN-009`:** the deterministic suites are still distributed rather than
      gathered, and contract test 8's cancellation-race half remains `BRN-008`'s journey. **DO NOT COMMIT.**
  - **The idempotency scope was three dimensions of the contract's five, and the gap was a disclosure.**
    `BRN-007`'s outstanding item said "`Idempotency-Key` scoping per principal and credential rather
    than per client". The migration that created the table keyed on `(workspace_id, operation,
    api_major)` while its own comment claimed the contract's five — so the omission **read as
    implemented**. What it permitted: a local profile has exactly ONE workspace
    (`DEFAULT_WORKSPACE_UUID`) shared by every enrolled client, while each client resolves to its own
    principal. Two clients presenting the same key collided, and `replayed_run` resolved the original
    run by workspace only — so the second client was handed the first client's `run_id` **and its
    `conversation_id`**, a handle onto another client's conversation. The keys are guessable in
    practice: the CLI derives one from the clock and its process id.
    - Fix is two dimensions and a check: `000007` rebuilds the table with `principal_id` and a
      `client_credential` **digest** in the unique index (`UNIQUE` is a table constraint in `000003`
      and cannot be dropped, so the table is rebuilt rather than altered); the adapter's insert, its
      lookup, and the in-memory double all name the principal; and `replayed_run` refuses a record it
      does not own. The credential is a digest because a secret has no place in a durable row — and
      recording it is what makes a rotation **observable** rather than a silent match.
    - **⚠ A guard I added is not covered by the test I wrote for it, and the mutation is what said
      so.** Disabling the ownership check in `replayed_run` leaves
      `one_clients_idempotency_key_cannot_replay_another_clients_run` **passing**, because the scoped
      lookup returns nothing for the second principal and the service creates a fresh run. The real
      guard is the `WHERE` clause, which the adapter test does falsify (dropping `principal_id` there
      fails it with `left: None, right: Some(...)`). The check stays as defence in depth and the code
      says plainly that its test does not cover it.
    - **A migration defect the tests caught, and the fix is the shape.** My first version did
      `ALTER TABLE ADD COLUMN ... NOT NULL DEFAULT ''` and then rebuilt — and the rebuild's `INSERT`
      ran **before** the backfill could matter, so the insert failed on the NOT NULL constraint. The
      rebuild now populates every column in the statement that creates the rows, which also makes a row
      with an unset principal **unrepresentable** rather than merely absent. Diagnosed by temporarily
      distinguishing the two insert failures from a generic conflict — a five-minute probe that a
      generic `Err(_) => Conflict` had hidden.
    - **A bulk edit silently changed a test's key.** Converting the record literals to
      `..idempotency_record()` also moved `"key-1"` out of the first claim, so the "reused key" test
      claimed with one key and conflicted against another. Caught by the test failing, not by review.
    - **`000007` rebuilds a table, which no migration here had done before, so the upgrade path got
      its own evidence.** `an_idempotency_record_written_before_the_scope_change_survives_the_rebuild`
      seeds a record at the **pre-`000007`** three-column shape and asserts the row survives and its
      `principal_id` is **attributed from the run it names** — exact rather than guessed, since
      `agent_runs.principal_id` is NOT NULL and the record's `run_id` is a foreign key to it. It also
      reads the created index back from `pragma_index_info` and asserts the six-part scope, which is
      the contract as a thing the database can be asked about.
      **All three assertions were falsified by mutation**, which is what makes them evidence: removing
      the backfill fails with `left: "unknown", right: <the run's principal>`; dropping `principal_id`
      from the index fails the scope assertion; and emptying the rebuild's `SELECT` fails the survival
      assertion with `RowNotFound`. The first is the one that matters most — the failure it guards
      against is an upgraded install silently losing a pending key and creating a **second** run for a
      retried command, which is the exact thing the key exists to prevent.
  - **The failing journeys were a stale binary, not a regression — and only rebuilding said so.** The
    first sweep returned `disconnect=1`, `policy=1`, `provider=1` with `the create was refused with
    409` (`idempotency.conflict`), which looked like the scope change rejecting a legitimate replay.
    It was not: `cargo test` builds test targets, not the `target/debug` binaries the journeys execute,
    so the journeys were still driving the **previous** round's `jarvisd` against a database the new
    migration had already upgraded. A rebuild made all eight pass unchanged. **The lesson is the order:
    a journey sweep is only meaningful after `cargo build`, and a `409` on a fresh key is a build-age
    symptom before it is a scoping symptom.**
  - 1157 workspace tests (+3: the adapter half, the service half, and the migration upgrade).
    All gates green, all eight journeys green. **DO NOT COMMIT.**
- [~] `BRN-010` Implement a visible, configurable model data-use, retention,
  locality, and telemetry policy that constrains routing and records provider
  disclosures/effective decisions. **The rules, the evidence predicates, and the
  selector are done; the input is not, and that gap is named below rather than implied.**
  Evidence: `jarvis_domain::model::policy` already held `PolicyRules`, the precedence
  merge (`merge_stricter`, strongest-layer-first so a later layer can only narrow), and
  the `RequestedDataPolicy`/`EffectiveDataPolicy`/`ModelRouteDecision` shapes — and none
  of it had a caller. `RejectionReason` was defined with **nothing able to produce it**
  and `ModelRouteDecision` with nothing able to construct it, so the contract's test 7
  ("routing rejection rather than silent relaxation") had vocabulary and no behaviour.
  `jarvis_domain::model::routing` is the selector: `select_route` takes a bounded
  candidate list plus a `RouteRequest` and returns the **first compliant candidate** or
  `model.policy_unsatisfied`. Three rules are structural rather than documented. **There
  is no path that relaxes a hard rule**: a candidate failing the policy or lacking an
  attested capability is rejected *with a reason*, and nothing downgrades a requirement
  because no candidate met it — which is also why selection is a filter and not a score,
  since a score could rank an incompliant candidate above a compliant one. **Every
  rejection is recorded, not only the winner**, because a decision naming just its
  selection cannot answer "why not the local model"; the list is bounded by
  `MAX_CANDIDATES` because it comes from a provider. And **retention and training use are
  candidate-attested, never inferred**: whether a provider documents bounded retention is
  a fact about its current published terms, so deriving `none_documented` from "it is a
  cloud endpoint" claims a documented finding from a category. A candidate with no usable
  note is now `provider_default` — a new `EffectiveRetention` variant meaning the
  provider's own default terms were accepted, which **documents nothing** — and a policy
  demanding a documented statement rejects it. **My first version got this wrong** by
  classifying an undocumented cloud candidate as `none_documented`, which is exactly the
  promotion the contract forbids: it made the least documented route read as the most
  careful kind, and a test caught it. **Two evidence predicates now exist and using one
  for both fails in one direction either way.**
  `Evidence::satisfies_hard_requirement_on` admits only `VERIFIED`, because a capability
  can differ between a provider's documented version and the one this repository pins;
  `Evidence::satisfies_data_policy_rule_on` admits `VERIFIED`, `DOCUMENTED`, and
  `OBSERVED`, because the contract names the labels that cannot satisfy a
  retention/training/residency rule (expired, `STALE`, `INFERRED`, `UNVERIFIED`) and
  official documentation is precisely what establishes a retention term. A
  `RouteCandidate::region` that is absent **fails** an allow-list rather than passing it,
  since a provider not publishing where it processes cannot be shown to be inside an
  allowed region, and refusing is recoverable while sending is not. 25 domain tests
  (that crate goes 191 -> 216), falsified by making the locality check always succeed,
  which fails 3 tests including the local-only-versus-cloud one. **Not done:** nothing
  constructs a `RouteRequest` from a **stored** policy yet, so `select_route` has no
  caller outside its tests — closing that needs the versioned, workspace-scoped policy
  store plus the `GET /api/v1/model-data-policy/effective` surface from this contract's
  list, and it is also what would let the run controller pass a real sensitivity ceiling
  instead of the permissive one `BRN-006` recorded. The telemetry and exception rules are
  likewise modelled and unenforced. This is stated because a tested-but-uncalled selector
  is the same shape as the dead `model_calls.route_decision_id` column this work exists to
  populate. **That column has since been given a writer** (`BRN-014`): the run's route is
  selected at creation, recorded on its budget, read back by the controller, and stamped on
  every attempt's row — so the selector, the decision store, and the column are all on the
  execution path rather than only on the diagnostic one.
  **The persistence half is now implemented.** `migrations/sqlite/000004_model_data_policy.sql`
  raises the schema to **4** (minimum reader stays 1 — every existing table and column is
  untouched) and creates the three tables this contract's `Persistence` section names:
  `model_data_policies`, `model_policy_exceptions`, and `model_route_decisions`.
  `jarvis_application::repository::policy` is the port, the SQLite adapter is
  `jarvis_infrastructure::storage::repositories::policy`, and `jarvis_application::testing`
  carries an in-memory double so a service test can register a policy without a database.
  **Immutability is structural rather than documented:** `insert_version` is an `INSERT`
  behind `UNIQUE (policy_id, version)`, so "changing rules creates a new version" is a
  constraint and not a convention — an `UPDATE` would silently rewrite the rules a past
  route decision was made under, which is the one thing the contract's historical-record
  rule exists to prevent. A duplicate is `storage.version_conflict`, the same typed outcome
  the run state machine uses, because it is the same fact: someone advanced the record since
  this caller read it; assigning the next version inside the store would make a concurrent
  update indistinguishable from a sequential one, and this contract requires
  `resource.version_conflict` for the former. **Rules are stored as JSON rather than as
  columns**, because `PolicyRules` has seven enums and four sets and a flattened schema would
  be twenty places for the stored form and the domain type to disagree with no single reader
  able to notice; a row whose rules cannot be reinterpreted is `Corrupted` rather than
  absent, since reporting it absent would let a caller create a replacement for a policy that
  is still there. **A decision stores its own copy of the policy reference**, because reading
  it through a live join would make an archived version's decision unreadable — exactly the
  case the historical-record rule is about. `load_active` orders by version descending, since
  a workspace may legally hold two active versions (reactivating an older one without
  archiving the newer), and the in-memory double implements the same rule because a
  difference between the two would be invisible to a test exercising either alone. An absent
  active policy is `NotFound` rather than an empty one: a call under no policy applies
  nothing, so the caller has to decide whether that is permitted, and it cannot decide if the
  store hands back `PolicyRules::permissive()`. 11 adapter tests including a real
  migrated-database round trip, plus the in-memory double. Falsified by changing the `INSERT`
  to `INSERT OR REPLACE`, at which point the immutability test fails because the second write
  silently replaces the row. **A defect this found in itself:** writing the migration without
  bumping `TARGET_SCHEMA_VERSION` made the daemon refuse its own database
  (`SchemaTooNew { found: 4, supported: 3 }`) and failed 8 tests — a migration and a version
  constant are one change, and the failure was loud only because the daemon checks.
  **The wiring half is now implemented too, and `select_route_explained` has a caller in
  production code.** `jarvis_application::policy_service::PolicyService` reads the **stored**
  policy and assembles the `RouteRequest` from it, which is what closed the gap named above:
  `jarvis_domain::model::routing::select_route` had a tested selector and no producer of its
  input. Two structural decisions. **The verdict and the explanation are one implementation:**
  `select_route_explained` returns a `RouteSelectionFailure` enum — `Refused(RouteRefusal)`
  versus `CandidatesUnbounded { offered }` — and `select_route` maps that to a `DomainError`.
  My own refactor first collapsed both arms into `PolicyUnsatisfied`, losing the contract's
  distinct code while a comment claimed the cases were distinguished, and **an existing test
  caught the lie**: a refusal means every candidate was evaluated, while an oversized set means
  none was, and reporting the second as the first tells a caller "your policy refused every
  model" when the truth is "you offered too many". **The evaluation day and the decision instant
  come from one clock reading** (`http::policy::evaluation_instant`), because two readings could
  straddle midnight and make the day and the instant describe different moments.
  `ModelProvider` gained `endpoint_class()`, since an adapter knows whether its endpoint is
  local, on a private network, or in a cloud and nothing above it can derive that — a provider id
  is not evidence either way, and guessing from the name or assuming the most permissive class
  would send confidential content to a cloud endpoint under a local-only policy.
  `GET /api/v1/model-data-policy` and `GET /api/v1/model-data-policy/effective` are implemented
  in `jarvis_infrastructure::http::policy`; the daemon composes the service and the
  `ProviderInventory` in `daemon.rs` from the **same** pool and provider the runs use, so a probe
  cannot report on a configuration no call would use. **A defect this found:** `rejected_view`
  rendered `RejectionReason`'s `Display`, which is operator prose, so the wire carried
  `"locality violated"` where the contract's response example shows the reason **code**
  `locality_violated`. Every other value in that module already went through a contract-spelling
  function; this one had none, and the handler test agreed with the defect because the code it
  reached was produced by the domain. Fixed with `rejection_reason_of`, and
  `every_rejection_reason_is_a_code_rather_than_a_sentence` now enumerates all nine variants and
  asserts each differs from `Display`, so the *class* fails rather than the instance. Six handler
  tests over a real migrated database, plus the spelling tests over every variant.
  **A second defect, recorded rather than fixed:** `CreateRunRequest.model_policy` is parsed and
  **never read**, so a client's stated policy ID/version does not reach `RunService::create` and
  the request is accepted with the field having no effect — indistinguishable to a client from
  "accepted and applied". Passing it through needs a plain params struct in
  `jarvis-application` (which cannot depend on `jarvis-protocol`) and is the next increment.
  **The write half is now implemented, which is what made that defect fixable rather than
  blocked.** `PUT /api/v1/model-data-policy` creates the next immutable version through
  `PolicyService::put`, and the decision worth recording is that **a request body cannot widen the
  policy in force**: the submission is merged with what is stored through
  `PolicyRules::merge_stricter`, which only narrows, so the write endpoint is safe without an
  approval step. That is contract layer 4 (task/run explicit restrictions) sitting beneath layer 2
  (workspace policy), and it is the rule the whole precedence order exists to express. Falsified
  by replacing the merge with the raw submission: **3 service tests and the HTTP test fail, and
  the failure body shows `approved_cloud_allowed` replacing `local_only`** — a widening that would
  have been invisible if the test had asserted only the status code. `expected_version` is a
  **precondition** rather than an instruction and `PutPolicyRequest` has no `version` field at
  all, because version identity is what keeps a past route decision explainable: a client that
  named the version could skip numbers or collide with an existing one, and a collision would
  report "your view is stale" to a caller that never held a view. A mismatch is
  `resource.version_conflict` (409) and **retryable**, while a contradiction is
  `jarvis.invalid_policy_layer` and **not** retryable — the first is fixed by re-reading, the
  second by editing the rules. That distinction was itself a defect: the code was first carried
  inside a `RepositoryError`, whose own `code()` reports its envelope, so a caller was told
  *storage* refused a *rule* mistake. **Two defects in this round's own work, both caught by the
  round-trip test:** `PUT`'s reply rendered the rules through the six-field *statement* shape, so
  `allow_fallback` was silently dropped from the daemon's own description of what it had stored;
  and `GET` omitted the same three fields, so a read could not be round-tripped either. Both now
  render through one nine-field function, and `a_read_after_a_write_describes_the_same_stored_row`
  asserts a write's reply and a subsequent read agree on every rule.
  **Still not done:** the inventory attaches no region, retention, or training-use evidence until
  `BRN-011` measures capabilities, which makes a policy demanding **documented** evidence refuse
  every candidate rather than having a claim inferred for it. The exception table's earlier
  "has a table and no code" note is now closed — see the exception paragraph below.
  **The create-run reference is now read, resolved, and enforced.** `RunService::resolve_policy`
  loads the named version (or the workspace's active one when none is named) and records both the
  reference and the resolved ceiling on the run, in `RunBudget`, so the run's own row explains
  *why* content was held back without re-deriving it from a policy that may since have been
  archived. **The ceiling then reaches context assembly**, which is the half that makes this more
  than bookkeeping: the controller previously passed a hardcoded permissive value and said in a
  comment that it did so because the policy merge was not built. Falsified by restoring that
  hardcoded value — **the confidential content reaches the provider and the run completes**, which
  is exactly the leak the ceiling exists to prevent. Three rules are structural: a **named**
  version is honoured exactly and a miss is refused rather than substituted, because falling back
  to the active policy would apply rules the caller did not name; an **absent** ceiling is not a
  permissive one, so a run in a policy-less workspace records **no** policy while the manifest
  still records every label — "a policy permitted this" and "nobody configured a policy" are
  different facts and only the first is a decision an operator made; and the ceiling is read from
  the **budget** rather than re-read from the store, because a policy edited mid-run would
  otherwise change the ceiling a running run is judged against, so two steps of one run could be
  held to different rules. The field is **optional**, forced by the architecture rather than
  chosen for leniency: the policy identifier is derived from the workspace and the workspace is
  resolved server-side, so a client cannot name the active policy without first reading it, and
  requiring it would make every run uncreatable on a fresh installation. Every existing harness
  and the CLI submitted `{"policy_id":"scripted-test","version":1}` — an identifier that is not a
  valid `ModelDataPolicyId`, so a policy **no workspace could hold** — and it went unnoticed only
  because the daemon ignored the field; reading it turned that fiction into a 422 across four
  tests, the CLI, and an E2E harness. **A defect this found in itself:** `agent_runs.error_code` is
  read back, is a column in the schema, and is **never populated** — the transition `UPDATE` sets
  `state`, `version`, `completed_at`, and the waiting columns but not `error_code`, so a failed
  run's own row cannot say why it failed. Same "a column that can never be correct" class as the
  round-16 `deadline_at`/`budget_json` finding; recorded as `BRN-012` below rather than fixed
  here, because closing it needs the code to travel on the transition rather than on the event's
  reason string.
- [x] `BRN-012` Persist the failure code on a terminal run transition, so a failed run's own
  row explains its outcome. Found while adding the policy-ceiling test above:
  `agent_runs.error_code` existed, was selected by `run_columns!()`, was read into
  `StoredRun::error_code`, and was **never written** — the transition `UPDATE` in
  `jarvis_infrastructure::storage::repositories` set `state`, `version`, `completed_at`,
  `waiting_kind`, and `waiting_ref` only. The consequence was that `GET /api/v1/runs/{id}`
  reported `error_code: None` for a run that failed, which left the contract's run read unable to
  provide the "public result/error summary" it requires; `RunView::error_code` existed to carry it
  with nothing to carry. Evidence: the code travels on the **write**, not on the event's reason
  string — `RunWrite::failed_with(TerminalOutcome::failed(code))` and a `Step::failed` constructor
  that **requires** a code, so a failure cannot be written without one and a cosmetic edit to a log
  label cannot change a client-visible identifier. `RunWrite::is_consistent` now also requires a
  `Failed` target to carry an outcome, which is what the adapter relies on: it binds `error_code`
  on **every** transition and `NULL` otherwise, because the column is the *current* outcome rather
  than a history — a run that failed and recovered must stop reporting the earlier failure. The
  code is built once from the same `ControllerError` the caller receives, so the row and the
  response cannot disagree. Recovery uses the `error_code` `RecoveryAction` already computed for
  its event payload, so the two cannot diverge either. Falsified by removing the adapter's
  `error_code` bind: `a_failure_outcome_reaches_the_stored_row_and_a_retry_clears_it` fails.
  The controller's own tests use the in-memory double, so the adapter carries this rule's coverage
  — which is why the regression test lives in `repositories_tests.rs` beside the other
  column round-trips. **Two rules are asserted in both directions:** a `Failed` transition writes
  the code, and a `Completed` one writes `NULL`; an adapter that bound it unconditionally would
  pass the first assertion and report a stale failure on the second.
- [x] `BRN-013` Implement the model-data-policy exception lifecycle and give the sensitivity
  ceiling its one enforcement point. Two contract obligations that shared no code and both
  needed the same layer.
  **The ceiling had no consumer at all.** `PolicyRules::maximum_sensitivity` was written,
  stored, read, merged, and compared by **nothing** — so a `local_only` policy permitting only
  `public` content selected a local candidate and sent `restricted` content through it. The fix
  is one check placed **before any candidate is examined**, because the ceiling bounds what may
  be sent by *any* route: a per-candidate reason would imply another candidate might carry it.
  New `RejectionReason::SensitivityExceedsPolicy` (`sensitivity_exceeds_policy` on the wire, so
  the wire vocabulary is decided in one place) and `RejectionReason::policy_rule`, which maps a
  refusal to the rule it is about — the bridge the exception relaxation is indexed by. Both
  sides of the boundary are asserted, because the ceiling is **inclusive** while the run
  deadline is **exclusive**, and two opposite conventions in one codebase is exactly where a
  later reader "fixes" one to match the other.
  **The exception lifecycle is now implemented end to end.** `jarvis_domain::model::exception`
  holds the typed model — `PolicyRuleKey` (nine keys), `ExceptionScope`, `ExceptionState`,
  `RequiredAssurance`, and `PolicyException` with `grant`/`state_at`/`is_usable_at` — the port
  and its SQLite adapter implement issue/read/list/revoke/consume, and
  `migrations/sqlite/000005_model_policy_exceptions.sql` brings the table to the contract's
  shape, raising the schema to **5** with the minimum reader left at 1. Five rules are
  structural rather than documented and each is falsified by a named test:
  - **A non-waivable rule cannot be relaxed.** `PolicyRuleKey::is_waivable` refuses the grant
    outright, and the selector **ignores** such a grant rather than honouring it — skipping
    leaves the rules stricter than the operator asked for, so it can never permit a call the
    unrelaxed policy would refuse, while refusing the whole evaluation would turn one unusable
    grant into a refusal of every candidate, including ones that never needed it. The check
    lives in both places because a record written by a *different* build, one that considered
    the rule waivable, must still not be honoured here.
  - **Sensitive or cross-border grants require step-up.** The requirement is derived from the
    rule (`locality`, `residency`) **or** from the scope naming a classification, recorded on
    the record, and enforced against the assurance the grantor actually held. The requirement
    is a required input rather than ambient state, because a grant whose own field says which
    assurance was required while nothing checked the grantor held it is the shape this closes.
  - **A grant is consulted only after the unrelaxed policy has refused, and only for the rule
    that refused.** This is what makes `relied_on_exception()` mean "this call *needed* a
    relaxation": a candidate that already complies records no grant, and a single-use grant
    offered to a candidate that did not need it is not consumed. **My first version applied
    every matching grant up front**, so a grant was recorded as relied upon merely because it
    existed — and my own test caught it (`a_candidate_that_complies_without_a_grant_records_no_exception`).
  - **Single use is a property of the record, not a flag on the call**, and the adapter's
    `UPDATE` carries `consumed_at IS NULL AND revoked_at IS NULL` in its `WHERE` rather than
    being guarded by a pre-read: a read followed by an unconditional write is the shape that
    lets two concurrent calls both consume one grant.
  - **Revocation and consumption are state changes, never deletes**, because the contract
    requires an exception to remain explainable after it stops being usable.
  **Two real defects the round's own tests found, one in the new code and one pre-existing:**
  `revoke`'s `UPDATE` lacked `AND revoked_at IS NULL`, so **re-revoking overwrote the first
  instant** and an audit would read the wrong moment; and
  `concurrent_transitions_on_one_run_never_fail_with_a_storage_fault` was **flaky** because it
  used terminal targets (`Cancelled`, `Failed`), so a writer that won the race made every later
  writer correctly report `jarvis.run_already_terminal` — a rule, not the lock contention the
  test exists for. Both are fixed and the second is now non-terminal-only.
  `PolicyService` gained `grant_exception`/`exception`/`exceptions`/`revoke_exception`/
  `consume_exception`, and `evaluate` offers the workspace's stored grants to the selector. The
  exception table's earlier note — "has a table and no code" — is therefore closed, and
  `docs/data/schema.md` now documents the three deliberate divergences from its own sketch:
  `state` is not stored (usability is a question about an instant), the scope is one JSON
  column rather than two, and `assurance` **is** stored so a change to the step-up policy
  cannot rewrite what a past grant meant.
  **Not done:** the HTTP surface for the exception lifecycle (the service exists, no route
  calls it), so `PUT`/`GET` for exceptions is the next increment; and a *replayed* decision
  still re-evaluates under current grants rather than inheriting the recorded one, which is
  what the contract means by "replaying a call re-evaluates current policy".
- [x] `BRN-014` Make the model-data policy govern a **real** daemon run, not only a
  diagnostic probe. Three holes that each looked closed from a unit test.
  **This is the "a guarantee that holds on the test path and not the production path" shape**,
  and it had three parts:
  - **The daemon built its run ports with `policies: None`.** Every handler test passed and
    `GET /model-data-policy/effective` answered correctly, while a real run resolved no policy,
    recorded no ceiling, and chose no route. A `RouteRequest` was never constructed on the run
    path at all. The fix gives `run_ports` the **same** `Arc<SqliteRepositories>` the policy
    surface uses (built once and cloned, not constructed twice), so a probe and a run cannot
    observe different stores. Falsified end to end: restoring `policies: None` and rebuilding
    makes the journey's new check answer `202` to a create the policy must refuse.
  - **The route was never selected for a run.** `RunService::create` now resolves the policy,
    then selects and records a route **before the run exists** — so a policy that admits no
    compliant candidate refuses the request (`403 model.policy_unsatisfied`) rather than
    creating a run that would have to be governed by something. The route travels on the run's
    own budget as `RunRoute { model, decision, exception_ref }`, so the model the run may call,
    the record that explains why, and the grant that permitted it are one value a later reader
    can see without loading the decision.
  - **The controller called `provider.models().first()`.** `RunController::resolve_model` now
    reads the stored route and requires the provider still to serve that model: a route naming
    a model the provider no longer serves is `run.no_model_served`, because falling back would
    perform an action no rule permitted. `selected_model` is retained **only** for the no-policy
    case. Falsified: making `resolve_model` return `selected_model(provider)` unconditionally
    fails four tests with `fixture-1` recorded where `fixture-routed` was authorized.
  Four smaller things the work forced, each a real defect rather than a tidy-up:
  - **`model_calls.route_decision_id` was a dead column.** It was in the schema, was named by
    both `SELECT`s, and had **no writer** — a literal `NULL` sat where the value belonged. It is
    now bound from `NewModelCall.route_decision` and re-parsed by `stored_model_call`, so it is
    readable rather than write-only. Writing it immediately exposed that `load_attempts`' query
    did not select the column at all, which a **pre-existing** test caught as
    `Corrupted { column: "route_decision_id" }` — the read and the write have to agree, and only
    a round-trip test can see that.
  - **`ProviderInventory` and `route_candidates` were two builders of the same candidate list.**
    The daemon's probe had its own loop, so the probe and the run could disagree about which
    models exist — the failure an operator would least likely see, because the probe would
    report a route the run never took. The probe now calls
    `jarvis_application::run_service::route_candidates`, so there is one builder.
  - **`RunBudget` lost `Copy`**, because a routed model owns a provider-qualified identifier.
    Four builders became non-`const`, and three call sites now clone explicitly. The one that
    mattered was a storage test that compared the stored budget against the same value
    afterwards — the assertion that makes the round-trip meaningful.
  - **`RunServiceError::PolicyUnsatisfied` maps to `403`**, not `400`: the body is well-formed
    and the caller is authorized — its own workspace's policy refuses the call — and a `400`
    would send a client to inspect its payload rather than its rules.
  **Not done, recorded rather than fixed:** `AuthenticatedClient` hardcodes
  `AuthenticationAssurance::Standard`, so **no HTTP client can ever be Elevated** and every
  step-up exception is un-grantable over the wire — that needs an owner decision and a step-up
  challenge, not a code change. The other limit this round recorded — `ModelCallRequest` carrying
  no model — is closed by `BRN-015` immediately below.
- [x] `BRN-015` Name the routed model on the wire, so a data policy constrains the **call** and
  not only the record. Round 29's recorded limit, closed in the round that found it.
  **The gap:** `ModelCallRequest` had no `model` field, so the controller honoured the routed
  model for *which adapter it called* and for what the `model_calls` row recorded, while the
  request the adapter received named no model at all. A provider was free to answer under
  whatever model it defaults to, and the run was then recorded as answered by the routed model —
  the same "a reader and no writer" shape as the dead `route_decision_id` column, one layer out.
  Invisible to every test beside it because the **row was already correct**.
  The fix is a **required** `model` on the normalized request, set from the run's stored route,
  plus two enforcement points and one correction:
  - **The provider port refuses a model it does not serve** (`model.provider_no_route`). Falsified
    by removing the check: the test fails and the failure body shows the substituted model
    serving the call — `call.started` naming `fixture-withdrawn` while `fixture-1` produced the
    text, which is precisely the record-disagrees-with-reality outcome.
  - **`call.started` reports the requested model**, not `models().first()`. A frame is what an
    operator and an audit read, so echoing the provider's first model would let a run's own start
    frame name a model nothing chose.
  - **The field is required rather than optional**, and that is asserted: a request with no model
    must fail to parse. An optional field would have kept every existing fixture compiling while
    the policy stopped constraining the wire, which is exactly how this survived — the type
    accepted a contract example in which no model appeared. Falsified by making it optional (a
    *compiling* mutation, so the failure is the assertion and not a type error): the test fails
    with `model: None` parsed.
  **Two guards, because a roster is a claim:** `resolve_model`'s roster check refuses a withdrawn
  model before an attempt is made, and the adapter's own check catches a provider that lists a
  model and still cannot route to it. The second is driven through the whole controller, so a
  refused open is asserted to leave the run **terminal** — the defect class this controller has
  been fixed for repeatedly — and to attempt exactly once, since `NoRoute` is not retryable.
  **One test was fixed rather than added.** `a_normalized_request_has_no_provider_extension_map`
  asserted the serialized text contained no `provider_`, which was a **proxy** for "no provider
  extension map" and stopped being one the moment `model.provider_id` — a required identity field
  naming *which* provider serves the call — appeared. It now parses the JSON and checks the key
  set against the whole portable surface, which states what it means instead of what it looked
  like. That is the lesson: **a substring assertion is a proxy, and a proxy that is adjacent to
  the rule it stands for breaks silently.**
  Docs corrected in the same change, since three places recorded the old limit:
  `model-stream.md` (the request example gains `model` and the prose says it is required),
  `model-gateway.md`, and `model-data-policy.md`. 929 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-016` Spend a single-use policy exception when a run relies on it. The exception
  lifecycle had a port, an adapter, an in-memory double, a service method, and tests, and
  **no production caller** — so a grant an operator marked single-use permitted an unbounded
  number of runs. The permissive direction of "a writer nothing calls", and the same shape as
  the dead `route_decision_id` column: correct in every test beside it, absent from the path a
  client takes.
  **The call goes in `RunService::select_route`**, and its position is the guarantee rather than
  an implementation detail: the decision is recorded **first**, the grant is consumed **second**,
  and the run is created **third**. Consuming before the decision would spend a grant on a
  selection that might then fail to record; consuming after the run exists would let a run be
  created whose grant is still unspent if anything in between failed. Here a failure leaves the
  grant intact and the request refused, and the decision is durable before anything is spent — so
  an audit can always name what the grant permitted.
  **The `single_use` predicate moved into the store's statement.** The adapter's consume
  `UPDATE` gained `AND single_use = 1`, so "is this grant single-use, and may it still be
  consumed" is one atomic predicate rather than a read followed by a write. A zero-row match on a
  repeatable grant is reported as **success**, not a conflict: the caller's grant is intact and
  still usable, and returning `Conflict` would tell it a standing grant was gone when nothing had
  happened to it. That is the false-refusal direction — the same class as reporting a stale view
  when the write never landed. The in-memory double implements the identical rule, because a
  divergence between the two is invisible to a test exercising either alone.
  **Four tests, and each direction is asserted.** A single-use grant a run relied on is spent; a
  spent grant **refuses the next create** rather than merely carrying a timestamp (a test that
  checked only `consumed_at` would pass against an implementation that consumed the record and
  then ignored its state); a repeatable grant is **not** spent and still admits a second run; and
  a grant that is merely *offered* — read on the route path but not needed — is not consumed
  either, since a store that spent everything it was shown would leave the workspace with nothing
  for the call that needs it. Falsified in both halves: removing the call makes the refusal test
  report a **`CreatedRun`** for a grant already used, and removing `single_use = 1` from the
  adapter's `UPDATE` stamps a repeatable grant as consumed.
  **The fixture had to be a locality grant against a local-only policy**, and the first attempt
  was wrong for a reason worth recording: the sensitivity ceiling is checked *before any candidate
  is examined*, so no grant can rescue content above it — the only relaxation a grant can change
  here is per-candidate, and a locality grant is the accessible one. The locality ladder is also
  ordered **most-restrictive first** (`LocalOnly` is strictest), so a grant must name a *more*
  permissive value than the policy's own for `max` in `relax_one` to widen it; naming a stricter
  one would leave the policy unchanged and the test would prove nothing.
  934 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-017` Record the finish reason the provider reported, so a truncated answer is
  distinguishable from a finished one. The third instance of one class, found by sweeping the
  schema for columns with no reader.
  **The gap:** `model_calls.finish_reason` had a schema column, a line in `docs/data/schema.md`, a
  **typed** field on `ModelCallOutcome`, and an adapter bind — and `RunController` wrote `None`
  while **no `SELECT` named the column** and `StoredModelCall` had no field for it. So "the model
  was cut off by its own token limit" and "the model finished" were one value on every read, while
  the domain deliberately retains an unmodelled provider reason as `FinishReason::Other` to keep a
  new one visible rather than flattened into a clean one. Both halves of the write/read pair were
  individually correct, which is exactly why nothing caught it.
  **The fix has three parts, and the third is the one the class is about:**
  - **Capture.** `ModelStreamEventKind::CallCompleted` was matched by the controller's `_ => {}`
    arm — the terminal frame was discarded apart from its usage. `DrainedTurn` gains
    `finish_reason`, and the terminal is now captured explicitly.
  - **A pattern-ordering bug I introduced and caught while writing it.** My first version had two
    arms: `CallCompleted { usage: Some(..), .. }` and `CallCompleted { finish_reason, .. }`. The
    first match wins in Rust, so any frame carrying **both** — which is what a terminal frame is —
    would have recorded its usage and silently dropped its reason. Merged into one arm that
    captures both, and `the_usage_on_a_terminal_frame_and_its_finish_reason_are_both_recorded`
    exists specifically because a test with usage **or** a reason alone passes against that bug.
  - **Read.** Both `SELECT`s name the column and `stored_model_call` parses it back into
    `FinishReason`. A value this build cannot reinterpret is `Corrupted` rather than absent,
    because reporting it absent would say the provider reported nothing when the truth is that
    JARVIS wrote something it can no longer read — the same rule the policy-rules column follows.
  `RecordedOutcome` replaces the bare `usage` parameter on the recording path, so the three things
  a provider can report (usage, reason, first output) travel as one named value rather than as
  transposable positional `Option`s; `Default` lets a failure path say "nothing was reported" by
  omission. The in-memory double records the same field, because a divergence between it and the
  adapter is invisible to a test exercising either alone.
  **Falsified in both halves:** removing the capture records `None` for a `Length` stop (and fails
  three tests), and removing the adapter's parse makes the column unreadable while the write still
  binds it — the exact write-with-no-read shape the round is about.
  **Not done, and named:** `first_output_at` was recorded by the controller and read by the adapter
  with **no producer of the instant** yet, because nothing in this build measures
  time-to-first-token — that measurement is `BRN-011`'s subject, so the honest state is a wired
  column and an absent producer rather than a dressed-up feature. **That producer now exists;
  see `BRN-043`.**
  940 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-018` Fix the cancellation path: an unenforced bound, a digest that folded only lengths,
  and a terminal event that carried nothing. Three defects on one path, found by following a
  declared constant to its enforcement point.
  - **`MAX_CANCEL_REASON_BYTES` was declared and enforced nowhere**, while `MAX_RUN_INPUT_BYTES`
    is applied one route above it. So a caller could post a multi-megabyte `reason` and the daemon
    would accept it. The bound is now checked with the same three rules the run input uses (empty,
    over the limit, NUL), and the check is asserted in both directions — an over-long reason is
    `400 request.invalid` and a reason **exactly at** the bound is accepted, so the check is a
    bound rather than an approximation of one. Falsified: removing the check accepts the
    over-long reason with `202`.
  - **The cancel idempotency digest was `reason.len()`** — folding the reason's *length* rather
    than its content. Two reasons of equal length were therefore the same request, so a caller
    reusing a key across two runs was told its second cancel was a replay of the first; and since
    a replayed cancel is a no-op **by design**, the second run was never signalled while the
    caller was told the cancel was accepted. A digest that cannot distinguish the inputs it
    claims to identify is worse than none. Falsified: restoring the length fold makes the test
    fail with the second cancel answered `Received` instead of a conflict.
  - **The reason never reached anything durable, and every activity event was written with
    `payload_json: None`** — so `jarvis_protocol`'s `cancelled_payload`, `failed_payload`, and
    `usage_payload` were all dead, and a client following the run's event stream learned that a
    run stopped without learning why. The **failed** case is now closed: the terminal event
    carries `{"code":…,"retryable":false}`.
  - **The reason travels on the `CancellationScope`**, not in a side table. The scope is the value
    the command actually signals and the value the controller already holds, so a registry keyed
    by run would need a second lookup, could disagree with the signal, and would be invisible to a
    caller holding only the scope. A child shares the reason rather than copying it (a controller
    working on a child must still report why), and a reason-less internal cancel **does not erase a
    reason already recorded**.
  **Two design constraints the work ran into, both real rather than inconveniences:**
  - **`jarvis-application` cannot depend on `jarvis-protocol`** (the flow runs protocol -> nothing
    app-side), and it has `serde_json` only as a **dev**-dependency. So the failed payload is
    hand-built, following `recovery::recovery_payload`'s precedent — legitimate only because every
    value comes from a **closed set** (a `&'static str` code and a literal boolean), so no escaping
    is needed. That leaves two definitions of one wire shape, so a cross-check test asserts the
    hand-built string equals `jarvis_protocol::run::failed_payload` for every terminal code — the
    same technique the duplicated event-name constants use.
  - **The cancellation payload is therefore still absent, and named rather than fudged.** Its
    reason is caller-supplied text, so a public event needs escaping; hand-rolling a JSON escaper
    is the ad-hoc string manipulation the architecture forbids, and adding `serde_json` as a real
    dependency is a research-gate change. I attempted it, hit the dependency wall, and **reverted
    cleanly** rather than leave a half-wired field. **SUPERSEDED — both the premise and the gap are
    closed, and the bullets below are the current state:** it turned out no caller-supplied text was
    on this path at all (the published reason was a `&'static str` label), and the payload is now
    published carrying **both** the caller's escaped reason and the controller's label.
  - **The cancellation payload is now published, and the reason it was not is the interesting part.**
    A cancelled run's terminal event carried **no payload**, so a client following the stream learned
    that a run stopped without learning **why** — while the same client reading `GET /runs/{id}` got
    the code from the row. The three ways a run stops have three different operator responses (the
    caller asked, the provider stopped it, the supervisor ended it), which is exactly why "it was
    cancelled" is not a sufficient terminal event. It now carries `{"reason":…}` from a closed set:
    `cancelled_before_start`, `cancelled_during_step`, `cancelled_after_output`,
    `cancelled_before_acceptance`, `cancelled_during_delivery`, and
    `provider_reported_call_cancelled`.
  - **The recorded blocker was false, and it was the second time in three rounds.** The note said the
    reason "is caller-supplied text, so putting it in a public event needs escaping, and hand-rolling
    that would be the ad-hoc string manipulation the architecture forbids" — with the corollary that
    closing it "needs a serializer at this layer or a sanitised reason at the boundary." Checked
    against the code, every clause is wrong: the reason a cancellation publishes is `Step::reason`, a
    **`&'static str` literal from a closed set**, while the genuinely caller-supplied cancel reason is
    held on the HTTP layer's `CancellationScope` and **never reaches this layer at all**. So nothing
    needs escaping, no serializer is needed, and `jarvis_protocol::run::cancelled_payload` already
    took a `&str` and escaped it with `serde_json::json!` — meaning even if caller text had been on
    the wire, the fix was to pass it through rather than to hand-roll anything. **The lesson is now
    recorded rather than the fix alone**: a "this needs escaping" claim is a claim about *which value*
    is on the wire, and it has to name that value. The previous round's false blocker was the same
    shape — "blocked on axum's sse feature" when that feature does not exist — so both are recorded
    together, because the pattern is the finding. **And the claim above was itself half wrong, which
    the next bullet records: the caller's reason was on this path after all, and it was reaching
    nothing.**
  - **A second, smaller defect fixed while wiring it:** the pre-acceptance failure path passed its
    **failure** label (`deadline_exceeded`/`provider_refused`) as a cancellation's event reason. No
    payload carried it, so it was invisible; publishing a reason is what made it wrong rather than
    merely untidy. A cancellation now uses a cancellation label on every path.
  - **The caller's reason now reaches the terminal event, which means I had to correct my own previous
    round.** That round published the controller's closed-set **label** and recorded, as a finding, that
    the caller's text "never reaches this layer" — so no escaping was needed and the round was a clean
    win. **That was false, and checking it was the round's work.** `CancellationScope::cancel_reason()`
    exists precisely so the caller's reason is reachable, and the contract's own sentence says *"The
    reason travels to the terminal event"* — meaning **their** reason. Worse, the caller's reason was
    reaching **nothing** at all: nothing in the controller read it, and no durable row held it, so a
    cancelled run's record could not answer the question the cancel endpoint had already asked the
    caller. The payload is now `{"reason":<caller text>,"label":<controller label>}` — the caller's
    escaped text, and the label a client can branch on without escaping concerns. Observed on the wire:
    `"payload":{"label":"cancelled_during_delivery","reason":"operator stopped it: see ticket #42"}`,
    with the `:` and `#` surviving intact.
  - **A broken escaper silently reproduces the exact bug this round fixed, and that is how it was
    found.** Deleting the `"` escape from `escape_json_string` leaves the whole unit suite green and
    fails only the end-to-end assertion — and what the wire showed was not garbled text but
    `"payload":null`. `render_event` in `http/runs.rs` parses a stored payload with
    `.and_then(…ok()).unwrap_or(serde_json::Value::Null)`, a deliberate choice to hand a client a
    detectable `null` rather than an invented object. The consequence here is that an escaping defect
    degrades *exactly* to the symptom being fixed: a terminal event that says a run stopped and nothing
    about why. Two lessons, both already in this file's history — a masking layer turns a corruption
    into an absence, and the assertion that catches it is the one that reads the **round-tripped value**
    rather than searching the raw frame for a substring. A `contains` check on the unescaped text would
    have failed on a *correct* implementation; one on the escaped text would have passed for a payload
    that dropped the newline.
  - **The test was made deterministic rather than tolerant, which forced a design fact into the open.**
    Its earlier version used the scripted provider and asserted "cancelled **or** completed", because a
    scripted run reaches its terminal as fast as the executor yields, so the cancel raced it and the
    cancellation branch — the entire point of the test — was usually the one skipped. Switching to
    `GatedProvider` (which blocks *between* frames, a state a scripted provider cannot occupy) exposed
    why the race existed: the controller re-checks the scope at a state transition and **once after the
    stream drains**, so a cancel that lands while frames are being consumed is decided by that second
    check. With the gate, the cancelled terminal is now *required* (`!stream.contains("run.completed")`
    is asserted), so a regression that made cancellation unreachable can no longer be satisfied by the
    fallback branch.
  - **The escaper is hand-written and proven rather than trusted.** `jarvis-application` has no JSON
    dependency, so the caller's text is escaped by `escape_json_string`, following the same pattern the
    CLI and the diagnostics manifest already use. Unlike those, it is **cross-checked against
    `serde_json`** over adversarial input — every character JSON requires escaped, the short forms, a
    control character with none, a boundary character, raw non-ASCII, and a backslash-before-quote case
    that breaks a naive order — plus a round-trip parse. That is possible because `serde_json` is a
    **dev**-dependency here: the test can reach the oracle the library cannot, which is exactly what
    makes the assertion a measurement of the escaper instead of a restatement of it. **Falsified** by
    forgetting to escape the backslash, which fails with the offending input named.
  - **An internal cancellation publishes no reason, deliberately.** Where a cancellation arrives as a
    `ProviderError` rather than from a scope, nothing knows who asked, so the label is published alone —
    inventing a reason for a cancellation nobody requested would attribute a decision to somebody who
    did not make it.
  - **The lesson, recorded because it was learned twice in three rounds**: "this needs escaping" and
    "blocked on a dependency" are claims about a *specific value* or *specific feature*, and they have to
    name it. This payload contains one value that needs no escaping and one that does; my previous round
    got both wrong while presenting the conclusion confidently. 1136 workspace tests (+1). **DO NOT
    COMMIT.**
- [x] `BRN-019` Publish the `run.usage` event, so the contract's required first-slice event type
  exists at all. Found by grepping the contract's **minimum event type list** against the code.
  **The gap was structural, not a missing line.** `run.usage` is one of the contract's minimum
  first-slice event types, `jarvis_protocol::run::usage_payload` had no caller, and the repository's
  **only** way to write an activity event was `transition` — so an event that reports something
  without changing state was *unwritable*. A client following the stream could not learn what a call
  consumed without making a second read; a unit test could not have caught it, because the port had
  no way to express the write.
  - **A new `RunRepository::append_event`**, which appends one activity event and **does not**
    advance the run's `version`. That is the design point rather than a detail: a version is the
    optimistic-concurrency token a *transition* states it expects, so bumping it for a report that
    changed no state would make two workers' transitions refuse each other for a write neither of
    them made. The adapter's unique constraint on `(run_id, sequence)` is what makes a gap
    impossible, and a taken sequence is `storage.conflict` rather than a transport fault.
  - **Emitted only when the provider reported at least one counter**, with each counter omitted when
    unreported. `Usage`'s counters are `Option` because **unknown is not zero**, and a public event
    is a durable statement: publishing `0, 0` for a provider that said nothing would record a
    measurement nobody made. The event names its `call_id`, or two calls in one run would be
    indistinguishable.
  - **Both spellings of the contract string are cross-checked.** The application layer cannot depend
    on `jarvis-protocol` and has no JSON *dependency*, so the event type and payload shape exist
    twice — a local literal and a hand-built string, against `event_type::USAGE` and `usage_payload`.
    A test asserts the event types are equal and the field names and values agree, which is the
    technique this workspace applies to every duplicated contract string. It also asserts the one
    thing the protocol builder *cannot* express: an unreported counter is absent rather than zero.
  - **The protocol builder's signature documents the difference**: it takes plain `u64`s, so it can
    express counters but not their absence — which is exactly why the hand-built shape is needed and
    not a duplication for its own sake.
  **Falsified:** removing the emission publishes no usage event (the test fails with zero found);
  the real-daemon journey asserts both directions — an event naming its call when one is published,
  and **no** event for the scripted provider that reports no usage.
  955 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-020` Record which runtime executed a run. The schema sweep that found `BRN-017` and the
  same shape again, one table over: `agent_runs.runtime_id` and `runtime_version` were named by
  `docs/data/schema.md` and by **no code at all**.
  **The gap:** `CreateRunRequest.runtime` is a **required** field, and the create handler did check
  it — an unsupported runtime is `422 request.semantic_invalid`, with a test asserting exactly that.
  What the handler never did was *record* it: the validated value was dropped and both columns stayed
  `NULL` on every row. So the check was real and the record was absent, which reads as coverage: the
  refusal test passes, the field is documented, the columns exist, and the architecture's resume step
  ("validate runtime identity/version") has nothing to validate against. Same class as the dead
  `route_decision_id` column and the unenforced `MAX_CANCEL_REASON_BYTES`: a value that stops at the
  boundary it was named for.
  - **A new `RunRuntime { id, version }` on the application's run repository**, with a validating
    `new` and a `native()` helper. Both fields are bounded, non-empty, and NUL-free, because both
    reach a `TEXT` column and SQLite cannot bound one — so the rule lives at the single place a run is
    built rather than in each adapter. Both are checked: a validation that covered only `id` would let
    an empty *version* through, and an empty version is what made "recorded" indistinguishable from
    "not recorded" in the first place. Falsified: validating only `id` lets an empty version through.
  - **The constructors default it to the native runtime** rather than taking it as an argument, and a
    `with_runtime` override is what a multi-runtime build would use. The default is *true of this
    build* rather than a convenience: the handler refuses every runtime this build cannot serve, so
    native is the only one that can reach a row. It also keeps a run from existing in a state where
    the columns stay `NULL` because a caller forgot.
  - **The version is the build's own (`env!("CARGO_PKG_VERSION")`), never the caller's.** A client
    cannot claim to be running a runtime version it is not, and the version a resume compares is the
    one that actually executed the run. The wire contract version answers a different question —
    which *protocol* a frame speaks — since a runtime's behaviour can change without its wire shape
    changing.
  - **The stored field is an `Option` while the created field is required**, and the asymmetry is the
    point: a row written before these columns had a writer has no truthful value to give, and
    reporting corruption for it would fail every existing database on upgrade. A row carrying **one**
    of the pair is corrupt rather than absent — half an identity cannot be validated against
    anything.
  - **The `jarvis-native` literal exists twice** (`jarvis-application` cannot depend on
    `jarvis-protocol`), so a cross-check test asserts the recorded id and
    `jarvis_protocol::run::NATIVE_RUNTIME` are one value — the technique used for every duplicated
    contract string here.
  - **The handler test reads the row back through the repository**, not just the response. That is
    what makes "the field reached storage" falsifiable, and it matters because the defect was in the
    handoff: the service could store a runtime correctly while the handler never told it one.
  **Falsified three ways, each compiling:** dropping the two `INSERT` binds fails the round-trip and
  the handler test; reading `runtime_id` twice in the `SELECT` also fails both — and an earlier
  version of the handler assertion (`!version.is_empty()`) **passed that mutation**, so it was
  strengthened to name the build version. That near-miss is recorded because a loose assertion beside
  a correct implementation is the thing that lets the next mutation through.
  **Still open in this class:** `agent_steps` columns `attempt_count`, `input_fingerprint`,
  `input_ref`, `output_ref`, `timeout_ms`, `next_attempt_at`; `agent_runs` columns
  `context_manifest_id`, `plan_summary_ref`, `result_ref`, `error_ref`; and `granted_by`/`scope_ref`
  on the exception and step tables. The sweep is cheap (`grep` each column name across `crates/`) and
  has now found four defects, so it should continue.
  963 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-021` Enforce the media type on every command, in the contract's own envelope. Found by
  grepping the contract's **minimum-code table** against the code — the sweep that found `BRN-019`
  one table over, and the first time this list has been read as a checklist.
  **The gap was two defects wearing one code's name**, and the split is the whole finding:
  - **`runs::create_run` and `runs::cancel_run` read the body as raw `Bytes`** and parse it by hand.
    That is deliberate and correct — it is what lets a malformed body answer the shared envelope
    instead of the framework's plain text — but `Bytes` applies **no media-type rule at all**, so a
    perfectly valid JSON command sent as `text/plain` was **accepted with `202`**. Verified live
    against a real daemon for `text/plain`, `application/x-www-form-urlencoded`, and a request with
    no `Content-Type` at all: all three created runs.
  - **`policy::put_active_policy` took `axum::Json`**, so the framework *did* refuse the same
    request — with a bare `415` whose body was the plain text
    ``Expected request with `Content-Type: application/json` ``. That violates "every refusal on
    this surface uses this envelope" and is the **third instance of one class**: round 5 fixed the
    empty-body `404` fallback and `tower_http`'s plain-text `413`, both for this same rule.
  - So `415 request.media_type_unsupported` was listed in the contract and returned by **no route**,
    for two different reasons. **A code that no code produces is invisible in both directions.**
  - **One check answers both:** `require_json_content_type` at the router, which produces the
    contract's code *and* is outermost with respect to the extractor — so the framework never gets
    the chance to emit its own text. The policy handler also moved to the hand-parsed shape the run
    handlers use, so a **malformed or unknown-field** body answers the envelope too; that half was
    untested before and is now covered by a test that drives four unparseable bodies.
  - **The predicate is axum's own, restated, not guessed.** `axum 0.8.9`'s
    `src/json.rs::json_content_type` accepts `application/json` or an `application/…+json` suffix, so
    `application/merge-patch+json` works and `text/json` does not. Restated rather than delegated
    because the framework's version is only reachable through a `Json` extractor, and using one
    would mean reading and discarding a body just to obtain a rejection. A contract test holds the
    two to the same rule over 13 spellings.
  - **Absence is inside the rule, not outside it.** RFC 9110 defines no default `Content-Type`, so
    an unlabelled body declares no format to be wrong about; refusing it would break every plain
    JSON client that omits the header while catching nothing. This also required the rule to be
    applied to `GET`s at all — a `GET` sends no `Content-Type`, so a rule that refused its absence
    would refuse every read on the surface.
  - **Ordering is inside authentication**, so an unauthenticated request is told about its
    credential first. The contract puts authentication before body handling, and the order is
    otherwise unobservable because both refusals share the envelope — only the `code` distinguishes
    them, so both directions are asserted.
  **Falsified four ways, each compiling:** moving the check outside authentication fails the
  ordering test; `|| true` in the predicate fails the refusal test with the exact defect in the
  failure body (**a `202` and a created run**); removing the layer entirely fails it too *and* makes
  the function dead, which clippy denies; reverting the policy handler to `Json` fails the envelope
  test with `Failed to parse the request body as JSON: …` — the framework's text. And a fifth: making
  the absent-header branch refuse fails **14** tests, every one a read, which is what settled the
  absence rule.
  **Verified on the real daemon and the real client:** the journey asserts all three refusals and
  the absent-header direction, and `jarvis ask` on a fresh profile still answers with exit `0` — the
  new layer is the first thing a real client meets.
  **Still open in this class:** `auth.invalid` and `auth.scope_denied` are in the same table and are
  returned by no route either. The daemon uses `auth.credential_rejected` for every authentication
  failure, which is a deliberate privacy decision (unknown, malformed, and revoked credentials must
  be indistinguishable), and `auth.scope_denied` has no consumer because no route authorizes at a
  scope finer than the workspace yet. Both need an owner decision: either the table's entries change
  or the code grows a route that can produce them. `request.rate_limited` is the fourth unreferenced
  code and is correctly absent — no rate limiter exists and `CON-004` owns connectors.
  **RESOLVED by `BRN-027`**, which took the owner decision in the only direction the facts allow:
  the two rows are gone, `auth.credential_rejected` and `resource.version_conflict` are in, and a
  parity test now holds the table and the surface to the same set.
  968 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-022` Enforce the event stream's `Accept` requirement, and make the reference client send
  it. Found by reading the contract's **Required Contract Tests** list as a checklist and then
  following a sentence in the endpoint description that nothing implemented.
  **The gap was two defects that hid each other, which is why neither was visible:**
  - `run_events` took the header map and read **only** `Last-Event-ID`, so the contract's
    "requires `Accept: text/event-stream`" was decoration. A client that sent
    `Accept: application/json` was served an event stream it cannot parse, and the mismatch
    surfaced in the client rather than at the boundary that could name it.
  - The CLI — which is the **reference client** for this surface — sent no `Accept` header at
    all. It worked solely because the daemon did not check. A permissive server and a
    non-conforming client are indistinguishable when driven against each other, so every test
    that exercised one against the other passed while both were wrong. **Fixing only one half
    would have broken the CLI against the other.**
  - And the CLI built these headers in **two** places (`runs events` and the follow loop
    `jarvis ask` uses), so a fix at one call site would have left `ask` silently non-conforming.
    Both now go through `event_stream_headers`.
  - **The predicate is a pure function** (`accepts_event_stream_value`) so the rule is testable
    over any spelling: a comma-separated media-range list, parameters ignored, `*/*`, `text/*`,
    and a bare `*` all permit; `application/json`, `text/html`, and a malformed range do not. The
    subset of RFC 9110 is small on purpose — a full content-negotiation implementation is not
    needed to answer "does this client permit the one representation this route has".
  - **Absence is inside the rule**, matching how this surface already treats an absent
    `Content-Type`: a request stating no preference can be served the only representation there
    is. **Falsified: refusing absence breaks 4 existing tests, every one a read of the stream.**
  - **Ordering: the media type is decided before the resume position**, and that is observable
    because the two refusals use different codes (`request.invalid` vs `stream.replay_unavailable`).
    A request that fails both must hear about the request, because a resume position is only
    meaningful to a client that is about to receive a stream.
  - **The refusal is `400 request.invalid`, not `406`.** The contract's minimum-code table has no
    `406`, and inventing a status/code pair is a protocol change rather than an implementation
    detail; `request.invalid` is what this surface already uses for a bad request header.
  **Falsified three ways, each compiling:** disabling the check fails the refusal test *and* the
  ordering test, the second showing the resume refusal reported first; refusing absence fails 4
  reads; removing the CLI's header line fails the client test — and the client half needed its own
  assertion because **what a client sends cannot be observed from the daemon**, which is exactly
  why this pair survived.
  **Verified on the real daemon and the real client:** the disconnect journey asserts the server
  half in both directions, and `jarvis ask` — which follows a run by polling this route — still
  answers with exit `0` against the enforcing daemon.
  **Still open:** `auth.invalid` and `auth.scope_denied` remain in the minimum-code table and are
  produced by no route (from `BRN-021`) — **closed by `BRN-027`** — and `MAX_SOURCE_BYTES` in `diagnostics` documents "the
  maximum number of bytes this process reads from a discovered JSON file" while `std::fs::read`
  allocates the whole file — a declared bound with no enforcement point, which is the class
  `BRN-018` and `BRN-021` both belong to.
  972 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-023` Make the size bounds bounds **on what is allocated**, not on what is retained.
  Found by the standing `MAX_*` sweep — the technique that found `BRN-018` — and it found four
  sites, one of which was a real memory hazard.
  - **`MAX_LOG_BYTES_PER_FILE` bounded the member and not the read.** `read_log_tail` did
    `std::fs::read` of the whole log and then sliced the tail out of the result, so a
    multi-gigabyte log was read into memory in full — **inside the support bundle**, the one tool an
    operator runs when something is already wrong. The cost is now proportional to what is retained:
    a new `read_tail_window` seeks to the window and reads only that.
  - **`MAX_SOURCE_BYTES` was declared and enforced nowhere**, while its doc comment said "the maximum
    number of bytes this process reads from a discovered JSON file". Every read of that file was a
    `std::fs::read`: the client's `discover` (on **every** CLI command) and both `lifecycle::discovery`
    reads, the second of which runs during **shutdown** on a path whose ownership is not yet
    established — so the file is the untrusted one. The constant moved to `client`, where the reading
    happens, and `diagnostics` re-exports it, so there is one value with one enforcement point rather
    than two names for one idea.
  - **The parser's own bound cannot substitute.** `DiscoveryFile::parse` checks `MAX_DISCOVERY_BYTES`,
    but it takes a `&[u8]`: by the time it can compare a length, the allocation has already happened.
    A bound on a *resource* has to live at the read, which is a different layer from a bound on
    *content*.
  - **A declared bound with a stated number and no enforcement point reads as coverage** — the
    constant exists, the doc comment is specific, and nothing consults it.
  **Two mistakes of mine, both instructive:**
  - **My first test asserted the wrong layer, and passed against the bug.** Both discovery tests
    drove `discover`/`remove_if_owned` and asserted the *outcome* — which is **identical** with the
    fix reverted, because `std::fs::read` returns the bytes and the parser then rejects them. A bound
    on a resource cannot be proven by an assertion on a result. Verified by restoring the unbounded
    implementation and watching the test pass, which is the only way that class of mistake is found.
    The assertions now drive the reader itself, and the reverted implementation fails both.
  - **Reading only the window makes the head partial by construction**, so the old early-return
    (`if text.len() <= MAX`) took the whole-file branch and kept a fragment — caught by a
    **pre-existing** test asserting the tail begins on a record boundary. The fix was to stop
    *inferring* partiality from a length comparison and have `read_tail_window` **return the offset
    it started at**, so the fact is stated rather than derived. A function that returns a window
    should say where the window begins.
  **Falsified four ways, each compiling:** the window read starting at zero fails three diagnostics
  tests including the offset one; and reverting each discovery reader to `std::fs::read` fails its
  own test. Verified on the real daemon: both E2E journeys pass and `jarvis ask` answers, so the
  bounded reads did not change any observable behaviour.
  **Still open:** `auth.invalid` and `auth.scope_denied` remain produced by no route (`BRN-021`) —
  **closed by `BRN-027`**, which removed both rows and added the two produced codes the table was
  missing — and the unreferenced-column list from `BRN-020` is unchanged — `agent_steps`
  (`attempt_count`, `input_fingerprint`, `input_ref`, `output_ref`, `timeout_ms`, `next_attempt_at`)
  and `agent_runs` (`context_manifest_id`, `plan_summary_ref`, `result_ref`, `error_ref`). Those are
  tables with no port at all, so they belong to the tool-fabric milestone rather than to a bound
  sweep.
  976 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-024` Decide the step-up rule from the assurance the caller **proved**, not one it was
  told. Found by following the two "recorded gap" doc comments the exception module carries, and
  the first one turned out to be describing a different defect than the one that existed.
  - **`GrantRequest` carried a `granting_assurance` field**, documented as "supplied by the HTTP
    layer from the authenticated client, which is the only place it is trusted". That was a promise
    nothing kept, and it was checkable: **no route grants an exception**, and the assurance already
    sits on `RequestContext` beside the `principal_id` it belongs to. So the step-up rule — the one
    thing the field existed for — was compared against a value the caller filling in the request
    chose. A caller could claim `Elevated` and obtain a durable relaxation of exactly the rules the
    contract requires a challenge for. **The trusted copy was never the one being read**, and the
    fix is to delete the untrusted one rather than to keep them in step.
  - **`grant_exception` now reads `context.assurance`**, which the server resolves from the
    credential, and a map (`required_assurance_of`) refuses `Guest` outright: a guest proved no
    identity, and a grant is accountable to a principal. Treating it as merely "standard" would let
    an anonymous caller create a durable record of a policy relaxation and be **named** on it.
  - **Two new error variants, because one code cannot name three remedies.** `Unauthenticated`
    reports `auth.credential_rejected` (`401` — the code this surface already uses, so a client's
    existing handling applies), and `InsufficientAssurance` reports the contract's own
    `model.exception_required` (`403`). Collapsing them to the domain's single
    `model.exception_required` would tell an operator to inspect a policy for a request that never
    authenticated.
  - **The existing test was asserting the wrong actor, and had to be rewritten rather than
    adjusted.** `a_grant_needing_step_up_is_refused_at_standard_assurance` set the request's <!-- citation-history -->
    assurance field, so it proved the *domain* checked whatever it was handed while the value it
    checked was the caller's to pick — a green test over a hole. It now varies the one thing a real
    request cannot forge, and the same `GrantRequest` is refused from a standard context and
    accepted from an elevated one.
  - **Four existing tests then failed, and each failure was correct**: every one granted a
    **locality** exception, which crosses a boundary the contract requires step-up for. They now
    present an elevated context — the path an operator actually takes.
  **Falsified two ways, each compiling:** replacing the derived assurance with a hardcoded
  `Elevated` fails both the step-up test and the guest test; and collapsing `InsufficientAssurance`
  onto the `401` status fails the mapper test with `left: 401, right: 403`.
  **The docs claimed the opposite of the truth**, and both were corrected in the same change:
  `exception.rs` said the type "has no producer above the domain" and the contract said
  "`AuthenticatedClient` hardcodes `Standard`, so a step-up exception is un-grantable over the wire".
  What is actually true is narrower and still open: the rule is now enforced server-side, but **no
  route grants an exception at all**, so the entire lifecycle is reachable only from inside the
  process. Adding the route needs a step-up challenge and an owner decision.
  978 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-025` Sweep doc comments that assert a caller exists. Found by counting the references of
  every public function in the workspace and noticing three whose count was **1** — their own
  definition — while each carried a doc comment stating what a caller does with it.
  **A doc comment claiming a caller is a claim a grep can falsify, and nothing checks it.** The
  three were not the same kind of problem, and telling them apart was the work:
  - **`event_matches_run`** — "A free function rather than inline, so the check the transaction
    relies on is separately named and testable." Nothing called it, and the transaction **could
    not**: `transition` builds every lookup from `event.run_id` itself, so there was no comparison
    against "the run the transition moves" anywhere. The rule read as enforced because a function
    named after it existed. **Deleted**, and the reason it cannot be enforced at that layer is now
    recorded where the lookup happens — `RunTransition` carries no run identity at all, so the
    adapter has nothing to compare against and the obligation sits with the caller that holds the
    `RunRef` (the create path *can* state and check it, and does, in `insert_run`).
  - **`model_ref_of`** — "Exposed so a caller that only needs the provider/model pair does not have
    to construct a `StoredModelCall` to get it." `stored_model_call` rebuilds exactly that pair and
    its revision inline, and that is where every read of a stored call goes. A second
    implementation of one parse, with no caller. **Deleted.**
  - **`is_client_visible`** — "Returns whether `visibility` may be shown to an ordinary client."
    No caller either, but **not** a defect: the client-facing event page filters in SQL
    (`visibility = 'public'`) and the in-memory double filters on the variant, both stronger than
    routing either through this function. The real risk was different and I only saw it by
    thinking about what the missing caller implies — a third `EventVisibility` variant would
    compile here (correctly returning false) while the **SQL string literal** silently excluded it
    from every read. So the predicate stays and is now held to the two filter sites by
    `the_visibility_filter_is_the_public_variant_only`, which asserts the variants, their stored
    spellings, and the adapter's behaviour.
  **Falsified two ways, each compiling:** making the predicate accept `Operator` fails with
  `Operator must be false to a client`; deleting the SQL `visibility = 'public'` clause fails two
  tests and the failure body shows the operator event in a client's page.
  **The general lesson, now recorded:** a doc comment that says "so a caller can…" is a claim about
  the codebase, and it is exactly as falsifiable as a schema column with no writer — but it is
  invisible to the compiler, to clippy, and to the docs gate, which checks links and IDs rather than
  whether a sentence is still true.
  979 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-026` Make the error envelope's `request_id` real. Found by counting references:
  `ErrorEnvelope::with_request_id` had exactly **two** — its definition and one unit test — so
  **no production caller existed** and every refusal on the run and policy surfaces shipped
  `request_id` as absent, while `common-conventions.md` promised the field and
  `local-control-api.md` named it a requirement for internal failures specifically. The
  contract's own example is a `tool.permission_denied` refusal, which says the field is not
  reserved for 500s: a refusal is the case a client most often needs a handle to report.
  - **Fix.** `require_authentication` mints one server-derived id per authenticated request,
    stores it as the `RequestIdValue` request extension, and returns it as the
    `jarvis-request-id` response header. `RequestIdOf` is the infallible extractor over it;
    `error_response_for(request_id, …)` is the shared builder, and plain `error_response`
    delegates with `None` so the two remain distinguishable. Every handler on both surfaces
    threads the id into its refusals **and** into `context_for`. The media-type layer, which
    runs inside authentication but ahead of the handler, now attaches the id it can already
    see rather than ignoring it — found by writing a test for a claim I had just put in the
    contract.
  - **Second defect, found while wiring it.** Both `context_for`s minted their **own** UUIDv7
    instead of using the request's, so even once a response carried an id the daemon's
    diagnostics were filed under a different one — the correlation the contract promises would
    silently return nothing. The context id is readable: `ScriptedProvider` stamps it into
    `ProviderMetadata.request_id` on the `call.started` frame, so it reaches an adapter.
  - **Layer order is now specified, not implied.** The identifier's presence is evidence the
    request authenticated, so it is minted *after* the credential check. Six refusals precede
    it and carry none, with the field absent rather than invented: the `413` body limit, the
    browser-`Origin` and forwarded-header `403`s, the `400` authority refusal, the `426`
    version negotiation (inside the middleware but ahead of the credential), and the `401`
    itself. Both the contract and `the_id_is_present_exactly_where_a_credential_was_verified`
    now state this, and the test asserts the inside cases *and* the outside ones.
  - **Falsified five ways, each compiling:** ignoring the id in `error_response_for` fails 5
    tests; a constant id fails 2 (including "identifiers must not repeat"); `context_for`
    ignoring its argument fails 1; the header minted from a fresh id rather than the stored one
    fails 2; and trusting a caller-supplied `jarvis-request-id` header fails
    `a_caller_cannot_name_the_request_it_is_answering` — the id must be server-derived, or a
    caller can aim an operator's search.
  - **Two pre-existing checks asserted byte-equality and were corrected, not deleted.**
    `an_unknown_run_is_not_found_and_a_malformed_id_is_indistinguishable` and step 4 of the
    policy-surface journey both compared whole bodies. The property they guard — a caller
    cannot tell which identifiers exist — is carried by the code and message, so both now
    compare apart from `request_id` **and** assert the two ids differ, which keeps the
    exemption from absorbing a genuine per-request difference.
  985 workspace tests, both E2E journeys green. **DO NOT COMMIT.**
- [x] `BRN-027` Make the minimum-code table and the surface name the same set. Found by inverting the
  sweep four earlier rounds had run in one direction. `BRN-021` counted each code in the table
  against production code and found four produced by nothing, then **recorded** the two it could not
  justify as needing an owner decision; `BRN-022` and `BRN-023` inherited the note and left it open.
  Running the sweep **the other way** — every code the surface returns, against the table — shows
  the table was wrong in *both* directions at once.
  - **A document that contradicted itself.** The table's `401` row said `auth.invalid`. Two
    paragraphs below, the same document's layer-order section names `auth.credential_rejected` as
    the `401` this surface returns — and that is what the daemon produces. So a client writing
    handling for a failed credential would key on a code it can never receive, and every test the
    daemon does have asserted the code the table omits. **`auth.invalid` was a name for a response
    nobody sends, sitting beside the name of the response everybody receives.**
  - **And the reverse omission, which the one-directional sweep could not have seen:**
    `resource.version_conflict` is produced on this surface (`PUT` with a stale `expected_version`,
    and the run's idempotency conflict) and was **not in the table at all**. A client could not know
    the status is `409`/retryable from the contract, only from a test.
  - **The decision `BRN-021` deferred, taken in the only direction the facts allow.** `auth.scope_denied`
    stays unproducible and its row is gone: no route authorizes at a scope finer than the workspace,
    so authentication answers "is this the owner" and nothing narrows what an owner may do. Adding a
    route to satisfy a table row would be building a control to fit a document. `auth.invalid` is
    deleted as **redundant**, not unproducible — one name for one response.
  - **`request.rate_limited` is kept and labelled `reserved`**, which is a correction to my own
    first draft: I wrote "every code in the table is produced", which was the very overclaim this
    round exists to fix. It has no limiter (the connector platform, `CON-004`, owns rate limiting),
    so the `429` row states its shape before a limiter exists while the prose says a client must not
    read "documented" as "will occur".
  - **Two of the produced codes are invisible to a source scan** — `idempotency.conflict` and
    `resource.version_conflict` travel on an error type's `code()` rather than as a literal here —
    so `CODES_CARRIED_BY_ERROR_TYPES` names them and says why the scan was not widened to other
    crates (that would find every code in the workspace and stop describing *this* surface).
  - **The test scans the surface's own source from `CARGO_MANIFEST_DIR`** and takes only the
    production half of each module, because a test may legitimately name a code it does not produce
    and counting its own assertions would make the test assert itself. It asserts it scanned at
    least three modules and parsed at least fourteen rows, so a broken scan fails rather than
    passing vacuously — the failure mode a fixture test exists to prevent.
  - **The table parser needed a second attempt, and the first failure was informative:** scanning the
    whole document for "must not list `auth.invalid`" **failed against my own corrected contract**,
    because the prose explaining the removal names the code. Rows are now extracted by shape
    (`| 401 | `code` | no |`) so a reworded paragraph cannot pass as a table.
  - **Falsified three ways, each compiling:** deleting the `resource.version_conflict` row fails with
    `["resource.version_conflict"]`; restoring `auth.invalid` in the table fails with
    `auth.credential_rejected` unlisted; and isolating the other direction, a table row for
    `auth.invalid` fails with `auth.invalid must not be a listed code`.
  986 workspace tests. Both doc gates green. **DO NOT COMMIT.**
- [x] `BRN-028` Test the authentication cases the contract claims, and stop claiming a control it does
  not have. Found by reading the contract's own authentication section against `RegisteredClient`.
  - **A stored field that cannot exist.** The contract said the daemon stores "a one-way verifier,
    client ID, creation time, **scopes**, and revocation state" and that authentication "reject[s]
    credentials from another profile, revoked clients, and **unknown scopes**". `RegisteredClient`
    has four fields and `enroll_owner_client` constructs all four; **there is no scope anywhere in
    the repository's auth code**, and `identity-workspaces.md` places scoped sessions in later work
    ("exchanged for a scoped session") — which this contract already says requires a separate
    device-enrollment contract. So both sentences described a control that does not exist, and a
    client or reviewer would read them as current. Corrected to state that the credential is
    **owner-wide and carries no grants**, with the scoped-session model explicitly deferred.
  - **An evidence claim wider than its tests.** The Milestone 1 note said test 2's four cases —
    "missing, malformed, wrong-profile, and revoked" — were covered. Two were: the
    indistinguishability test compared missing against a **wrong but well-formed** token (the good
    token with one character appended), and a separate test covered revocation. **Malformed and
    another-profile were untested while being asserted as covered.**
  - **Why those two were worth driving rather than inferring:** they take a different internal path.
    A malformed credential fails while **decoding, before any comparison**, so `verify` returns
    `CredentialError::Malformed` — a *different variant* from the `Rejected` every other case
    produces. And another profile's credential is well-formed and hashes correctly; it is simply
    unknown to this daemon, which is the case the "another profile" rule exists for and the one a
    hostile local user would actually present. New
    `a_malformed_or_foreign_credential_is_indistinguishable_from_a_missing_one` drives both against
    the no-credential response, since that is the response a caller can already produce and the one
    indistinguishability must be *from*.
  - **The handler test could not carry the malformed case on its own, and that shaped the fix.**
    `require_authentication` maps *any* `CredentialError` to one response, so a mutation that
    propagates the variant leaks through the **body** while the handler test's body-equality still
    holds. `ClientRegistry::authenticate`'s doc states the collapse ("unknown, malformed, and revoked
    credentials all produce this same value"), so the assertion now lives where the collapse happens:
    `a_malformed_credential_collapses_to_the_same_rejection_as_a_stranger` asserts the **code** over
    four malformed spellings plus a foreign credential.
  - **Falsified twice, and the second mutation was the discriminating one.** Propagating the error at
    the middleware fails the equality test *and* the new one, but only on the body — which is the
    weaker signal. Propagating it at the **registry** leaks `jarvis.credential_malformed` and fails
    **only the targeted test**, with the handler equality staying green — exactly the discrimination
    that shows the two assertions cover different layers.
  988 workspace tests. **DO NOT COMMIT.**
- [x] `BRN-029` Reconcile the local control API's endpoint list, its capability list, and the create
  response with the route table. Found by reading the contract's "Public Endpoints" block against the
  router, then following each concrete claim beside it. **Three defects, and the third is a test
  defect that made the second invisible.**
  - **A `PUT` the endpoint list never named.** The block listed seven routes; the router serves
    **ten**. `GET`/`PUT /api/v1/model-data-policy` and `GET /api/v1/model-data-policy/effective` were
    absent from the one section whose purpose is to enumerate the surface. A client reading only this
    contract could not discover them, and a reviewer checking the surface against the document would
    find the document short rather than the code. All three now appear, with a note that their
    request/response shapes are owned by `model-data-policy.md`.
  - **A `Location` header the contract promised and the daemon never sent.** The create step said
    "Successful creation returns `202 Accepted`, a `Location` header, and: …", and
    `created_response` built only a body. A `202` without `Location` gives a client a status it can
    see and no way to address what it created, so every client would build the path from the id —
    which is how two clients come to disagree about a URL the daemon owns. Now emitted from the same
    builder as `links.self`, so the header and the body cannot name different resources.
  - **A capability list that had drifted in both directions at once, hidden by a test whose name
    claimed the check it did not make.** The contract's status example advertised
    `runs.create/read/cancel/events` while the daemon advertised **only `system.status`** — so a
    client reading the example would call an operation the daemon never advertised, and a client
    reading the daemon would not learn the run routes exist. The protocol test was named
    `the_status_example_names_the_capabilities_the_daemon_actually_serves` and its comment said "the <!-- citation-history -->
    daemon's own route table must agree", **but it only read the contract's example and compared it
    to a literal list** — the document against itself. It could never see the daemon, and
    `jarvis-protocol` cannot: it does not depend on `jarvis-infrastructure`.
    **A test that states a property in its name and checks a weaker one is worse than an absent
    test, because the name is what a reviewer trusts instead of verifying.** The daemon half now
    lives in `jarvis-infrastructure::http`'s
    `the_advertised_capabilities_cover_every_routed_operation`, beside the router; the protocol test
    is renamed to what it actually does and keeps the document-vs-protocol check.
  - The advertised set is now `SYSTEM_CAPABILITIES` (seven entries) rather than a literal inside the
    handler, so a route added without its capability is a test failure instead of an omission a
    client discovers by probing. The contract documents the field as **advertised, not
    authoritative** — it is not consulted before serving a route, so it can grant nothing.
  - **Falsified four ways, each compiling:** omitting the header fails
    `a_create_names_the_created_resource_in_a_location_header`; setting `Location` to the *events*
    path fails on the header/body comparison with both values shown; shrinking the advertised set to
    `system.status` fails naming `runs.create` and its route. A fourth mutation that rebuilt the
    path with `format!` **survived, and that is correct** — it produces the identical string, so it
    is behaviourally equivalent rather than a defect. Recorded because a surviving mutation needs a
    reason, not a reflex.
  - **Journey A extended to read response headers**, which it could not before: the `request` helper
    exposed only status and body, so the header half of a contract was unobservable end to end. It
    now asserts `Location == links.self` against the **real daemon**, which is the only evidence that
    the header survives serialization.
  990 workspace tests. Both doc gates green. **DO NOT COMMIT.**
- [x] `BRN-030` Make startup recovery reach **every** interrupted run, and make a bounded read say so.
  Found by applying the previous round's technique — a test whose name claims a cross-check its body
  cannot perform — and then following the claim it hid.
  - **The test-shaped defect first:** `the_double_enforces_scope_like_the_real_adapter` claimed in its <!-- citation-history -->
    name to compare the test double against the SQLite adapter. It cannot: `jarvis-application` does
    not depend on `jarvis-infrastructure`, so the test drove the double and asserted against the
    double. Renamed to what it does. The same sweep found **nine comment claims of parity** inside
    `testing.rs` ("matching the adapter's ordering", "the adapter's unconditional bind") — each a
    claim the crate cannot check. Those comments are how the real defect below stayed invisible.
  - **The real defect, found by checking one of those claims.** `testing.rs` said its
    `incomplete_runs` was "matching the adapter's ordering" — and the adapter had
    `LIMIT MAX_INCOMPLETE_RUNS` while the double returned **everything**. A double that enforces
    *less* than its adapter is the dangerous direction: a test passes against behaviour the
    production store never produces.
  - **What the divergence was hiding is the point.** The adapter's bound is a *page size*, and
    `reconcile` read **one** page and stopped. Interrupted runs are ordered **oldest-first**, so the
    runs a single page leaves behind are the **newest** — and they stay non-terminal on that restart
    and every later one, because each pass recovers the same oldest page and reports success. A
    profile with more than 500 interrupted runs silently never recovers the most recent ones, which
    is the exact state this whole pass exists to prevent. The bound read as a memory optimisation and
    was in fact a correctness hole.
  - **Two further consequences, both invisible from the bound alone.** `is_complete` was
    `failures.is_empty()` — so a truncated read reported as a clean pass. It is now the conjunction
    with a new `incomplete_store` fact, because "no write failed" is not "no run was left". And
    `start` now **fails** on `incomplete_store` rather than logging a count, since readiness is
    defined as classification having completed.
  - **Design change: the read reports its own boundedness.** `incomplete_runs` returns a new
    `RecoveryPage { runs, bounded }`, and the adapter reads **one past** the bound so `bounded` is
    observed rather than inferred from `len() == MAX` — an inference that would silently become
    wrong if the bound moved, and that a store returning exactly its limit would satisfy. The double
    now enforces the same bound.
  - **The loop had to be able to conclude as well as continue**, and the reason is specific: a
    refused write leaves its run in the state the read looks for, so a full page that settled
    **nothing** would be re-read for ever — on the startup path. That judgement is now a pure
    function, `page_outcome(bounded, settled) -> Drained | More | Stalled`, because a wrong
    `continue` hangs and a wrong `break` strands runs; the pure function is the only way to assert
    both without one of them hanging the suite.
  - **A test-harness lesson worth recording.** Reversing the loop's stall branch was caught — but as
    a **hang that ran for ever**, not as a failing assertion, because the fixture's future resolved
    on first poll and `tokio::time::timeout` **cannot preempt a future that never awaits** on a
    current-thread runtime. Adding one `yield_now()` to the refusing fixture turned the same mutation
    into a clean 10-second `Elapsed(())` with a message naming the defect. Killing the hung run also
    left a stray test binary, which is what `BRN-008` recorded breaking the *next* build.
  - **Falsified two ways, each compiling:** reverting to a single-page read fails
    `recovery_drains_a_store_holding_more_than_one_page` with `abandoned: 500` against `503` — three
    runs stranded, exactly the shape of the production bug; and continuing on a stalled page fails
    `a_page_that_settles_nothing_...` with `Elapsed(())`.
  - Also fixed: the recovery read returning `RecoveryPage` rather than a bare `Vec` at **every** call
    site (adapter tests, the recovery tests, the daemon), and `MAX_INCOMPLETE_RUNS`'s doc, which now
    says why a bound here is a page size and not a total.
  993 workspace tests. Both doc gates + `--changed-file` green. **DO NOT COMMIT.**
- [x] `BRN-031` Collapse the duplicated default deadline, and delete a bound nothing could enforce.
  Found by the standing `MAX_*`/`DEFAULT_*` sweep, which listed every such constant with two or fewer
  references. **Three findings, one class: a value that exists in two places, or in one place it
  cannot act.**
  - **The same default, declared twice.** `jarvis_domain::run::budget::DEFAULT_RUN_DEADLINE_MS` and
    `jarvis_application::run_service::DEFAULT_RUN_BUDGET_MS` were both `900_000` with **nothing
    comparing them**, and the domain's copy had no production caller — so the *documented* default
    was one the daemon never used. Editing either alone would have left the default written to a
    run's budget disagreeing with the default the domain publishes, and no test could see it: each
    crate asserted its constant against its own `900_000` literal, which is the number written twice
    rather than an agreement. Collapsed with `pub use … as DEFAULT_RUN_BUDGET_MS`, so the value has
    one definition and every caller's spelling is unchanged. The alias is honest because
    `jarvis-application` may depend on `jarvis-domain` and does.
  - **A claimed agreement test that does not exist.** `MAX_OBJECTIVE_BYTES`'s doc said "a test in
    the daemon asserts the two agree" with `jarvis_protocol::run::MAX_RUN_INPUT_BYTES`. The daemon's
    only test checks profile source names, and **nothing anywhere compares the two** — nor can it
    from either crate: `jarvis-application` cannot depend on `jarvis-protocol`, and vice versa for
    the service's constant. The comparison now lives in `jarvis-infrastructure`, which depends on
    both: `the_objective_bound_is_the_one_the_wire_bound_enforces`. The failure it prevents is
    asymmetric and quiet — raising the wire bound alone would let the handler accept text the
    service then refuses with `422`, telling a caller its body was too long by a route that had
    already agreed to take it.
  - **A bound nothing could enforce.** `MAX_RETAINED_VERSIONS = 8` was referenced by **no code, no
    test, and no document**. It could not have a consumer: `plan_prune` removes one named version
    and refuses the active one and the rollback target, so there is no operation that could say "too
    many — drop the oldest", and the CLI has no prune subcommand. Deleted, with the reasoning
    recorded in place, because the safety property it gestured at is real and **is** enforced —
    structurally, by those two per-version refusals, and asserted by
    `the_active_version_and_the_rollback_target_are_never_pruned` and
    `a_prune_all_never_removes_every_path_back_to_a_working_version`. That is the fourth time this
    project has found "a declared bound read as coverage"; here the honest fix was removal, not
    wiring it to a fabricated consumer.
  - **Corrected a `TODO.md` claim that made the third finding look covered:** `FND-012` listed prune
    beside a CLI subcommand list that omits it, which is how a reviewer concludes the bound is
    enforced somewhere.
  - **Falsified (compiling):** raising `MAX_RUN_INPUT_BYTES` to 64 KiB fails the new agreement test
    with `left: 65536, right: 32768`. The alias needs no mutation — it is one definition, so there
    is nothing left to disagree.
  994 workspace tests — **one** new test, and this line said 996 for a while. Corrected, because the
  count is the kind of claim that gets read as evidence: I had written "+3" from the number of
  *findings* rather than the number of tests, and the two crates' own suites already covered the
  alias and the deletion (nothing to test — one has no second copy left and the other is gone).
  Both doc gates + `--changed-file` green. **DO NOT COMMIT.**
- [x] `BRN-032` Sweep public functions for the ones a doc comment claims a caller for and none calls.
  Found by counting every `pub fn`'s references — technique (3) — and reading the doc comment of each
  one whose only reference was its own definition. **Three dead functions, and the class is why they
  survived: a doc comment saying "so a caller can…" is a claim nothing checks.**
  - **`no_context_manifest()`** returned `None` for a run's `agent_runs.context_manifest_id`, with a
    doc saying "a caller that needs a manifest id now uses this so the field's absence is explicit
    rather than a hardcoded string appearing in two places later". **No caller existed** — not in
    production, not in a test, not in a doc — and it was `#[must_use]`, which is the tell: it
    marked itself as something a caller ought to heed while nothing heeded it. Deleted. When a
    manifest is genuinely persisted the `Option` belongs on the read of a stored run, not in a free
    function returning a constant.
  - **`SupportBundle::total_bytes()`** summed the included members' lengths, and the CLI prints the
    **archive** size from `export_bundle`'s outcome instead. A second size computation beside the
    real one is worse than an unused getter: it is the one a later reader reaches for when adding a
    "bundle size" line, and it would then disagree with what is printed for any bundle whose members
    compress. (This project writes stored entries, so the two agree *today* — exactly the
    coincidence that would keep a wrong implementation alive.) Deleted; `files()` stays, because the
    preview's own test reads it.
  - **`redacted_log_bytes(path, redactor)`** wrapped `read_log_tail` + `redact_text` for "a caller
    that already knows its source". No caller. **Checked before deleting rather than after:** the
    bundle path redacts log members itself (`BundleSource::LogTail => read_log_tail(item)`, then
    `redact_text`), so this was a second entry point to one guarantee, not the guarantee. The
    `clean-machine-smoke` canary — plant a real credential in a real log, export, assert the archive
    omits it — still passes, which is the independent evidence that no redaction path was lost.
  - **Falsification here is the compiler plus three journeys** rather than a mutation: deleting a
    function nothing calls cannot be shown to break a test, so the honest evidence is that the
    workspace builds, all suites pass, and the canary still reports "the exported archive omits the
    planted credential". Claiming a mutation would have been theatre.
  - **Also corrected my own previous `TODO` note:** `BRN-031` said 996 workspace tests; the real
    total was 994, and its one new test brought 993 → 994. I had written "+3" from the number of
    *findings* rather than tests. **A self-authored count is read as evidence downstream, which is
    why it is worth re-deriving instead of continuing the sequence.**
  994 workspace tests (unchanged — this round deletes code and adds no test). Both doc gates green;
  all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-033` Verify the schema-compatibility refusals the migrations table specifies. Found by
  reading `docs/data/migrations.md`'s startup-behaviour table as a checklist — the technique that
  found `BRN-021` — and asking which rows have evidence. **Three of five had none**, while the table
  reads as a specification of behaviour a reader would take as implemented.
  - **`DB newer than binary | Refuse writes/start, preserve state, explain update`.** Both halves
    were unverified: `StorageError::SchemaTooNew` is produced by `read_compatibility` and the daemon
    maps it to `StartupError::Storage`, but nothing drove a `start` into it. New
    `a_database_written_by_a_newer_binary_is_refused_and_left_untouched` seeds a database whose
    compatibility record names a future version, then asserts the refusal **and** that the record
    still names the future version — the second is what makes "preserve state" true, since a refusal
    that helpfully downgraded the record would leave the database claiming to be older than it is.
    Also asserts the message names the remedy, that the failure is not retryable, that discovery was
    not published, and that the lock was released so the operator can fix the binary and retry.
  - **`Checksum mismatch | Refuse readiness; require diagnosis`.** `run`'s own doc calls this out as
    why it does not pass `set_ignore_missing`, and nothing tested it. New
    `a_checksum_mismatch_is_refused_rather_than_applied_over` corrupts the recorded checksum of an
    applied migration (bytes flipped, not the row deleted — a missing row reads as "not applied" and
    would take the ordinary path) and asserts `jarvis.db_migrate`, not retryable.
  - **Two rows are left `not implemented` in the table rather than given a test.** `start` migrates
    unconditionally, so there is no branch for "explicit approval required" or "repair-required" to
    take — and a test of behaviour that does not exist is exactly the false evidence the column
    exists to prevent. **A table of states with no way to tell which are real is how a reader
    concludes a control exists.**
  - **A stale doc on the constant every reader checks:** `TARGET_SCHEMA_VERSION` said "Bumped to `3`
    by `000003_idempotency.sql`" while reading `5` — two migrations had been added without
    revisiting the sentence. A migration that forgets to bump the constant fails a test; a *doc*
    that forgets is silent. The doc now carries a version → migration table so the next omission is
    visible in review rather than in a downgrade.
  - **`has_pending`'s doc claimed a caller that does not exist** — "this is what lets startup decide
    between 'migrate safely' and 'report readiness false' **before it changes anything**", which
    `start` does not do (it migrates, then reads compatibility). Corrected to describe the intent as
    intent, keeping the function for what it actually is: the honest way for a test to read migration
    state without applying anything.
  - **Falsification, and one mutation that proved nothing.** Reversing the daemon's check order
    (compatibility before migration) is caught by seven *other* daemon tests, and by mine not at all —
    it still refuses. I recorded that rather than claiming credit for it. The mutation I wanted for
    the preserve-state assertion could not be expressed: placing the write after
    `read_compatibility` leaves it unreachable behind the early return, so the mutant never runs —
    a *mutation* error, not a test gap, and re-running it until it "passed" would have been a false
    result. Checked sqlx's own source to be sure of what I was asserting: the checksum comparison is
    **not** gated by `ignore_missing` (which short-circuits only the version-missing check), so the
    refusal my test drives cannot be disabled that way.
  996 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-034` Put the durable database in the **local** data directory, not the roaming one. Found
  by reading `docs/architecture/process-topology.md`'s platform-path table — the technique that
  found `BRN-033` — and checking each row against the pinned path crate instead of against the other
  docs. **A real data-integrity defect, in the one line that decided where the database lives.**
  - **`ProfilePaths::standard` used `ProjectDirs::data_dir()`, which is
    `{FOLDERID_RoamingAppData}` on Windows** — while the field's own doc said "the local,
    non-roaming data directory", while `log_dir` three lines below already used
    `data_local_dir()`, while this module's header documented the choice, and while three documents
    (the architecture table, the Rust-foundation evidence note, and `docs/data/`) all stated the
    non-roaming rule. **Four places agreed; the line that mattered disagreed.**
  - **Why it matters rather than being cosmetic:** a domain account's `AppData\Roaming` is copied
    between machines at logon, and on a slow-link profile is synced through a temporary offline
    store. A live SQLite file would travel with its `-wal` and `-shm` siblings — and a database
    opened on two machines while its write-ahead log is mid-flight is **corrupt**, not merely stale.
    The roaming choice also defeated the deliberate local/non-roaming split between `config` and
    `data`.
  - **No test could see it, and that is the transferable part.** `standard()` branches on the *host*
    platform, so every assertion in the module ran on Windows — the one platform whose answer was
    wrong — and `data_dir()` and `data_local_dir()` return the **same** value on macOS and Linux,
    where most of this project's work is reviewed. The pre-existing comment even said "Mutable state
    must be local, never roaming" while the assertion beside it only checked the path was non-empty.
  - **The check is now cheap and platform-independent:** the new test compares
    `data_dir()` against `data_local_dir()` **directly** rather than against a literal like
    `AppData\Local`. A literal would be Windows-only, which is exactly the shape that cannot see this
    class; two accessors of one API distinguish the two answers on the one platform that has two.
    **Falsified by reverting to `data_dir()`:** fails with
    `left: "C:\Users\...\AppData\Roaming\JARVIS\JARVIS\data"`, `right: "...\AppData\Local\..."`.
  - **Two more rows of the same table named nothing real**, and each is the kind a reader takes as
    already true: the cache row said "OS local cache/Jarvis" (not a path the pinned crate can
    produce, and not what the code does — it is `{LocalAppData}\JARVIS\JARVIS\cache`), and the logs
    row said `~/Library/Logs` / `$XDG_STATE_HOME/jarvis/logs` where this build has one log directory
    under the data root. **Verified by running a probe against the real resolver rather than by
    reasoning about it** — which caught an error in my *own* correction: logs and runtime are under
    `<durable data>/`, not siblings of it. The probe was deleted afterwards, because a probe that
    stays becomes a second path-resolution surface nothing owns.
  - **A second, independent false claim, in an evidence note.** `rust-foundation.md` said "Tests
    assert the exact resolved table on each native OS" — they did not — and separately claimed
    Windows "uses an owner/system-only DACL and **queries it back** before publishing discovery or
    credential material", contradicting the deferral bullet above it and describing enforcement that
    does not exist: `verify_directory_permissions` is Unix-only and the non-Unix
    `create_dir_owner_only` is a bare `fs::create_dir_all`. Both corrected to the narrow truth
    (containment is enforced and asserted; the ACL is inherited, not verified). **A false "tests
    assert this" is the claim that stops the next reader from checking.**
  - Added `the_documented_platform_table_matches_the_resolved_paths`, which asserts the *relational*
    structure the table publishes — data root equals the local accessor, database/logs/runtime under
    it, cache outside it, and the three subdirectory names — because those hold on every platform
    while a host-specific literal does not.
  998 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-035` Give the client-visible run-state vocabulary **one definition**, and check it
  against the contract for **values** rather than for shape. Found by working the schema sweep
  (`agent_steps` and `model_*` first, both honestly documented as unbuilt) into the enumerated
  `CHECK` constraints, then asking of the one that named a real type — `agent_runs.state` — whether
  the migration's own claim ("a state added there without a migration here would fail this CHECK")
  was **enforced by anything**. It was not: no test reads a migration, and the claim is a comment.
  - **The real defect was one step away.** The domain→wire projection
    (`jarvis_infrastructure::http::runs::wire_state`) maps the twelve domain states onto the seven
    the local control API exposes, and the contract states that set — "Initial states are
    `received`, `context_building`, `model_running`, and `responding`. Terminal states are
    `completed`, `failed`, and `cancelled`." Its covering test asserted the image's **cardinality**
    (`assert_eq!(unique.len(), 7)`) and three terminal names. **Any seven distinct strings satisfy
    that**, so renaming one arm — `context_building` to `context_built` — kept the count, kept the
    terminals, and left the daemon emitting a state the contract does not publish, with every build
    gate and all three end-to-end journeys still green. The set existed twice: as bare literals in
    a `match`, and as English prose in a paragraph nothing read.
  - **My first fix was vacuous, and the mutation said so.** I compared the projection's image
    against the protocol constants the projection is *spelled from* — so renaming the constant moved
    both sides together and the assertion stayed green (`INFRA_MUTANT=0`) while the protocol test
    caught it. A test comparing two things derived from one definition is the same failure as a
    double that shares the code's assumptions, one level down. **Replaced with the assertion a
    rename cannot move:** the *grouping* the contract promises in prose — planning is
    indistinguishable from context-building to a client, and the five work-outstanding states are
    indistinguishable from each other — which is a statement about domain values, not about strings.
    Re-falsified by regrouping them, which now fails. The document comparison lives in the crate
    that can read the document.
  - `jarvis_protocol::run::run_state` is the new owner of the seven names, next to the
    `event_type` constants that already did this for event names. The projection spells its arms
    from them, so the vocabulary cannot diverge by an edit at a call site.
  - New test `the_wire_state_vocabulary_is_exactly_the_one_the_contract_publishes` reads
    `docs/contracts/local-control-api.md`, extracts the two sentences' backticked names, and asserts
    **both directions** — a state the contract names with no constant fails, and a constant with no
    mention in the contract fails — plus that the two published sets are disjoint. The boundary test
    was **rewritten in place** rather than added to, so the count below rises by one rather than two.
  - **A second, larger instance of the same class found while fixing it.** Running `cargo doc` —
    which **no gate had ever done** — reported **21 unresolved intra-doc links**, two of them naming
    a type (`WireRunState`) that **does not exist anywhere in the workspace**, including the sentence
    explaining where the wire-state projection lives. Rustdoc reports these as warnings by default,
    nothing ran rustdoc, and no gate denied the lint, so a doc comment pointing a reader at a symbol
    nobody wrote read exactly like one that resolves. Fixed all 21; each crate root now denies
    `rustdoc::broken_intra_doc_links`; and CI gained a `cargo doc --workspace --no-deps` step, since
    a `deny` that never fires is decoration. **Not** run with `-D warnings`: that additionally denies
    `redundant_explicit_links` and `private_intra_doc_links`, which are style complaints about links
    that *do* resolve, and widening a gate past the defect is how it gets relaxed later.
    **Falsified:** inserting one broken link now fails the doc build (`101`) instead of warning.
  - **A tooling trap, recorded because it silently corrupted a file.** To mutate a constant I used
    PowerShell's `Set-Content -Encoding UTF8`, which prepends a **UTF-8 BOM** (`EF BB BF`) on
    Windows PowerShell 5.1 — confirmed by reading the first bytes back. The file still compiled, so
    nothing reported it; `cargo fmt` was the only thing that would have. Removed the BOM and switched
    mutations to `[System.IO.File]::WriteAllText` with `UTF8Encoding($false)`. `AGENTS.md` says never
    to edit via the terminal, and this is a concrete reason why beyond the stated rule. The
    `clippy::panic` denial also caught `panic!` inside the new test helper — `clippy.toml` allows
    `expect`/`unwrap` in tests but not `panic`, and this file's JSON tests already use
    `unreachable!` for the same case, so the helper now matches them.
  - Corrected the documents that described the projection as more checked than it was: the
    contract's "enforced by construction" bullet (`local-control-api.md`), `agent-runtime.md`'s
    projection note, and the earlier milestone `TODO` note whose "a test asserts it is both total
    and coarser" was true but read as though the values were checked.
  999 workspace tests (+1). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-036` Delete the maintenance-lock machinery nothing called, after its runbook step told an
  operator to use it — and correct the timestamp-ordering convention the same module relied on.
  Found by working technique (2) (schema columns → writer *and* reader) to the end of the sweep:
  `application_locks` is created by `000001_initial.sql` and, by a substring grep, had **no writer**.
  - **A substring grep of a column name is not a grep of the module that owns it.** The next check —
    grep the *module* — found `storage::lock`, a complete lease implementation with tests for
    acquisition, refusal, lease reclamation, and owner-checked release. So the correct statement was
    not "no writer" but "a writer **and** all its callers are missing": `acquire_lock` and
    `release_lock` appeared exactly once each, in the `pub use` line of `storage/mod.rs`. **A table
    plus a tested module reads as an implemented control**, and the tests made it read better than
    it was.
  - **And a document told an operator to use it.** `docs/data/migrations.md`'s Local Upgrade Flow
    began "Stop or drain daemon and **acquire profile maintenance lock**" — a step that could not be
    performed by any command, with no way for an operator to discover that. The guard that actually
    prevents two daemons sharing a profile is the held file lock in `lifecycle::InstanceGuard`
    (taken before startup work, asserted by daemon startup tests); the row-level lease is a
    *different* mechanism — a visible holder and reclaimable expiry — and remains unimplemented.
  - `storage/lock.rs` and its re-exports are **deleted** rather than kept in case: an uncalled
    module is a claim, and this one had a runbook and a schema heading restating it. The
    `application_locks` table stays (migrations are checksum-pinned and applied databases exist);
    the module doc now says plainly that the table has no reader and no writer, and
    `docs/architecture/storage-data.md`'s "advisory locks or lease rows coordinate singleton jobs"
    bullet is marked as a **target, not a description**.
  - **A second defect fell out of the deletion, and it was the more interesting one.** Deleting the
    module removed the workspace's **only** production call to `jiff::Timestamp::now()` — which
    `docs/research/integrations/rust-foundation.md` already asserted was "not used in library code"
    while it was, and which the deleted file's own doc called out as the thing this project avoids.
    Time now enters through `domain::clock::Clock` and the `SystemClock` adapter everywhere.
  - **I then made an evidence mistake worth recording.** To check timestamp handling I read
    timestamp strings out of `%TEMP%\jarvis-acceptance-gate-*` and reported "both shapes appear in a
    real database". **That database was not this project's** — probing it showed `daemon_instances`,
    `users`, and `workspaces` tables and no `application_locks`, while this repo creates
    `agent_runs`, `agent_steps`, `application_locks`, `conversations`. A directory whose *name*
    matched was treated as evidence. Re-derived everything from this repo's own code and tests,
    which is stronger anyway; **the finding survives, the citation does not.**
  - **The real timestamp finding, measured rather than assumed.** `UtcTimestamp`'s doc said the value
    "has exactly one string form", and `common-conventions.md` said RFC 3339 text "agrees with
    chronological order". A probe over the domain's own type refutes the second: `Z` fixes the
    offset but not the width, and since `.` (`0x2E`) sorts before `Z` (`0x5A`),
    **`2026-09-20T12:00:00.1Z` sorts *before* `2026-09-20T12:00:00Z`** although it is the later
    instant. So a `TEXT` `ORDER BY`/`<`/`<=` over these strings is not chronological and can name
    the wrong row — which is exactly what the deleted lease's `expires_at <= ?` predicate assumed.
    **No current code path depends on it** (`incomplete_runs` and exception `list` order by
    `created_at`/`issued_at` for a bound and a stable order, not chronology), so the defect is
    **latent, not active** — recorded as such rather than dramatized. I also got the *mechanism*
    wrong in my first correction: I claimed a zero fraction renders at a different width, the test
    refuted it (`jiff` **omits** zero digits, so `…:00.000Z` and `…:00Z` are the same string), and
    the convention and doc now state the measured rule with an assertion beside it
    (`the_canonical_forms_of_one_instant_are_equal_while_their_text_order_is_not`) so the warning
    cannot go stale silently.
  - Corrected `docs/contracts/common-conventions.md` (Time), `docs/data/migrations.md` (runbook),
    `docs/data/schema.md` (the `application_locks` heading), `docs/architecture/storage-data.md`
    (target vs description), `docs/research/integrations/rust-foundation.md` (two claims), and
    `time.rs`'s own doc.
  - **The count goes DOWN, and the arithmetic matters.** Deleting `storage::lock` removed its five
    tests (acquisition, refusal, lease reclamation, owner-checked release, scope independence) and I
    added one here, so **995** = 999 − 5 + 1. Written out because a falling count in a round that
    added a test looks like a typo, and because the alternative — keeping five green tests for a
    module nothing calls — is what made this defect look implemented in the first place.
  995 workspace tests (−4 net: −5 deleted lock tests, +1 new). Both doc gates green; all three
  journeys pass. **DO NOT COMMIT.**
- [x] `BRN-037` Make the sensitivity label round-trip test assert what its comment claimed, so drifting
  the **writer** cannot pass. Found by sweeping every `enum -> &str` mapping against its reverse
  parser — the pattern the domain documents as "an inverse and a test asserting the two agree over
  every variant".
  - **The two halves live in different crates, which is why the gap survived.** `Sensitivity::as_str`
    renders the label a message carries; `context_assembly::parse_sensitivity` reads it back in
    `jarvis-application`. Nothing could compare them directly, so the agreement rested entirely on
    one test — and that test **only iterated `PARSEABLE_SENSITIVITY_LABELS`**, a hand-written array,
    asserting each entry parses. It compared the parser against a copy of the parser's own arms. The
    array's doc even said its existence was what made "a new `Sensitivity` variant added without a
    label here a test failure" — so the comment claimed the check the body did not perform.
  - **The failure mode is a run that fails on a message the product labelled itself.** `as_str` has
    exactly one production caller (`run_service`, writing a message row). Add a variant, add its
    `as_str` arm, forget the parser: the writer stores `secret`, the reader returns `None`, and the
    run dies with `AssemblyError::UnlabelledMessage` — while every gate is green. **A comment saying
    "a test asserts this" is a claim a grep falsifies and nothing checks.**
  - The test now iterates the **domain's variants**, calls `as_str` for each, and requires the parser
    to read it back — plus asserts the array equals the domain's rendering element-by-element, and
    that the two sets have the same size in both directions.
  - **Falsified in the direction that matters, with a control.** Drifting the array
    (`restricted` → `restrictedx`) fails at "position 3 must be the label Restricted renders". But
    that is the easy direction, which the old test also caught. The decisive mutation drifts
    **`as_str`** (`restricted` → `restricted_content`): the new test fails with
    "restricted_content writes Restricted, so the parser must read it back", and I then **temporarily
    reinstalled the old test body and confirmed it passed that same mutant** (`OLD_TEST_WITH_WRITER_MUTANT=0`)
    — the difference between "a test exists" and "a test checks the thing".
  - **First mutation attempt was invalid and I discarded it.** My `Replace` of the enum body dropped
    its closing brace, so the failure was a syntax error (`unclosed delimiter`), which falsifies
    nothing — no logic changed. Redone with the edit tool so the file stayed valid. A mutation that
    fails to compile is not evidence; it is a typo with a red exit code.
  - **Also fixed a documentation drift I created in `BRN-035`.** I added a `cargo doc` step to the CI
    `lint` lane but did not update what the docs say CI runs: `docs/operations/ci-gates.md`'s "What
    Runs On Every Push" table and its local-gate list both omitted it. `AGENTS.md`, `CONTRIBUTING.md`,
    and `README.md` now list it too, with the reason it is not run under `-D warnings`.
  995 workspace tests (net 0: the corrected test replaced its own body). Both doc gates green; all
  three journeys pass. **DO NOT COMMIT.**
  - **Checked and cleared, so a later round does not re-investigate:** 17 `.rs` files in the working
    copy have CRLF endings while 94 are LF, despite `.gitattributes` declaring `*.rs text eol=lf`.
    That is **not** a defect and not caused by these edits — untouched CRLF files (`clock.rs`,
    `live_events.rs`, `context/manifest.rs`) report **clean** in `git status`, and the diff for the
    CRLF file edited here is surgical (`13 insertions / 5 deletions`, not a whole-file rewrite),
    because `core.autocrlf=true` and `.gitattributes` normalize the comparison. **Do not "fix" it:**
    rewriting endings would be a whole-file churn with no behavioural change, and it would show up
    as a suspicious diff in review.
- [x] `BRN-038` Give the credential file **one** reader and one validation rule, and bound what is read.
  Found by holding the multi-participant file sweep (a file written by one party, read by several)
  against the credential, and by asking of each rule whether it was implemented once.
  - **The same rule was implemented twice, and the two disagreed.** The credential file is one input
    read by two functions that are both reachable: `auth::load_client_credential` (the daemon, on
    every start) compared only the **length**; `client::read_credential` (the CLI and `doctor`)
    compared the length **and** the base64url alphabet. So a 43-byte file of arbitrary characters was
    accepted by the daemon's reader and refused by every other reader — whether an invalid spelling
    is acceptable is a property of the *input*, and it came out differently depending on which
    function looked at it. Two readers of one file is how that happens.
  - **And both allocated the whole file before checking anything.** Each did `std::fs::read` and then
    compared `len()`, so a `len` check guarded against malformed *content* while doing nothing about
    *how much was read*. That is the same defect `BRN-023` closed for the discovery file — whose
    bounded reader sits a few functions away in `client.rs`, and whose test already records the trap
    this round fell into once.
  - Fix: `credential::is_presentation_text` is the one rule, `credential::read_presentation_text` is
    the one reader (metadata check **plus** `take(bound + 1)`, so a file that grows between the two
    calls is still bounded), and `MAX_CREDENTIAL_SOURCE_BYTES = 256` is the one bound. Both readers
    now delegate, so their codes differ and their answers cannot.
  - **My first bound test was vacuous and the mutation said so.** I wrote 257 `'A'`s and asserted a
    refusal — and it **passed with the unbounded read restored** (`MUTANT_UNBOUNDED=0`), because the
    content rule rejects that file anyway, so nothing measured the bound. The discriminating input is
    a file that is over the bound while its **trimmed content is a valid credential**: a real
    43-character credential followed by whitespace, which `String::trim` accepts. The bounded reader
    refuses it; the unbounded one returns a perfectly good credential. **Re-falsified: fails with
    `std::fs::read` restored, passes with the bound.** A bound on a *resource* cannot be proven by an
    assertion on a *result*, and the input must make the bound the only thing that can reject it.
  - **The rule's divergence is falsified separately:** restoring the daemon's length-only rule fails
    the agreement test at "the daemon must refuse characters a credential cannot contain", while the
    alphabet check's own mutation is caught too. The agreement test asserts the two **agree** rather
    than that either is right, and gives both directions a case, so unifying them on the permissive
    rule would fail as well.
  - **Deliberately not folded together:** `is_presentation_text` could have been
    `base64_decode_unpadded(...).is_ok()`, but those answer different questions — *is this readable*
    versus *does this decode* — and merging them would let a future change to decoding silently
    loosen what is read. Their agreement is asserted by a test instead, so drift is caught rather
    than prevented by construction.
  - **A finding recorded rather than papered over:** `DaemonDescriptor.api_major` is documented as
    "the API major version **the daemon reported**", but its `From<&Discovered>` impl substitutes
    `crate::http::API_MAJOR` — the major *this client speaks* — because `Discovered` does not carry
    the field, and doctor then prints it as the daemon's. `DiscoveryFile::validate` never checks
    `api_major` either, so the published value is neither validated nor read by anything. Both majors
    are `1` today so the output is not wrong, but the label claims a fact the client path did not
    observe; the honest fix is for doctor to read `Jarvis-API-Version` off a real request. The doc
    now says which value it is on each path, and the test that called itself "the same descriptor"
    was renamed to what it actually checks.
  997 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-039` Dial the host the record published, and bind the family the client can dial. Found by
  following the client's transport, where the validated authority and the connected address were
  **two different values**.
  - **`request` checked the host and then ignored it.** It read `host`, refused anything that was not
    `127.0.0.1` or `[::1]`, and connected to a hardcoded `("127.0.0.1", port)` — so the value that
    passed validation never reached the socket. Three faults followed from one line: a record
    publishing `[::1]` (which `format_base_url`, `authority_of`, and `validate_loopback_url` **all
    support and all test**) could never be dialed; the `Host` header was built from the validated
    authority while the connection went elsewhere, so the client stated an authority it had not
    connected to; and the check `starts_with("127.0.0.1")` was a text prefix rather than an address
    test, admitting `127.0.0.10` and refusing `127.0.0.2` — wrong in both directions.
  - **The daemon bound IPv4 only**, which is what made the IPv6 half unreachable rather than merely
    broken: the whole stack intended dual-stack loopback except the one line that chose the socket,
    so a host with IPv4 loopback unavailable could not start at all. `bind_loopback` now tries IPv4
    first (so the common path publishes the same `127.0.0.1` as before) then IPv6, and still refuses
    any non-loopback result.
  - Fix: `client::dial_host` **parses** the authority as an address and dials *that*. Parsing rather
    than resolving is the security property as much as the correctness one — `TcpStream::connect`
    with a name goes to DNS and the hosts file, which is the one thing a loopback-only client must
    never do.
  - **A real-socket test, because no pure test could catch the original.** `dial_host` asserts what
    should happen; a test that binds a listener and drives `get_public` asserts what does. The IPv6
    half is the falsifying one: the old code accepted the record and then connected to `127.0.0.1`,
    so a listener on `[::1]` rejects it. Verified — the mutation fails with "the request reaches the
    IPv6 listener, which a hardcoded IPv4 dial cannot: Transport".
  - **The test immediately caught a bug I introduced fixing it.** Binding the parsed `IpAddr` to
    `host` made the `Host` header render unbracketed (`::1:53996`), which a real daemon refuses as
    `jarvis.host_not_allowed` — the body still came back, so only the assertion on the header the
    *server saw* could have found it. Kept as a separate name (`address`), with the reason recorded.
  - **A three-way inconsistency, recorded rather than unified.** "Loopback" is defined three times and
    they disagree: `validate_loopback_url` and `dial_host` admit `127.0.0.1` and `::1` only;
    `http::authority_of` and the `Host` check take whatever the daemon bound, so `127.0.0.2` would be
    accepted if the daemon bound it; and `bound.ip().is_loopback()` accepts all of `127.0.0.0/8`.
    Nothing is exploitable today because the daemon binds `127.0.0.1`, but one rule per concept is the
    project's own standard and this is three. Recorded as an open item rather than "fixed" by
    widening or narrowing whichever one I happened to be holding.
  1000 workspace tests (+3). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-040` Resolve a stream resume position against the **stream**, not against one page of it.
  Found by following `Last-Event-ID` from the header to the lookup, and asking what the lookup was
  actually searching.
  - **A page bound leaked out of the transport and became a claim about retention.**
    `resolve_resume` read one page of retained events (`service.events(context, run, 1)`) and
    searched it, while `MAX_EVENT_PAGE` bounds a **page** and a run's stream does not: the controller
    publishes **one durable event per streamed output chunk**, so a long answer passes 500 events as
    a matter of course. A client whose last-seen event was past that page got `409
    stream.replay_unavailable` — a code the contract reserves for a position the daemon *no longer
    retains* — while the event sat in the store. The comment defending the scan claimed scanning was
    "the honest way" because "a second lookup path could disagree with the stream about what is
    retained"; that is true of retention and false of *identity*, and the two questions are now
    answered by different things.
  - Fix: a `load_event_sequence` port method (workspace- and visibility-scoped, `NotFound` for a
    foreign run, `NotFound` for an absent event) plus `RunService::event_sequence`, so the handler
    asks "what sequence does this event have" instead of "is it in the first 500". The page scan also
    re-read a page on every reconnect to discover nothing.
  - **MY FIRST TEST DID NOT FALSIFY AND THE MUTATION SAID SO.** I appended **one** event at sequence
    510 and resumed from it — and the pre-fix page scan **passed** (`MUTANT=0`), because the page bound
    is on the number of *rows read*, not on the sequence value: a nine-event run has a first page that
    includes sequence 510. The test measured nothing. **The trigger is more than a page of events
    ahead of the target**, so the corrected test appends a full `MAX_EVENT_PAGE` of them and then
    **asserts the precondition it depends on** — that the target is absent from the first page — so a
    change to the bound or the fixture cannot silently make the case unreachable again.
    **Re-falsified: the mutant now returns `409` for a retained event.**
  - **And my first mutation was invalid.** I added the old scan as a *fallback* in the `Err` arm, but
    the new lookup **succeeds** for a retained event, so the mutant code never ran — a mutation that
    cannot execute proves nothing, the same way an unreached branch does not count as covered. Read
    where the mutant actually is on the path before trusting the result.
  - Two more `RunRepository` doubles (`StallingWrites`, `Unreadable`) needed the new method, which the
    compiler reported both times rather than letting one silently diverge.
  1001 workspace tests (+1). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-041` Answer a stale durable precondition with `409 resource.version_conflict`, and hold the
  table's `Retryable` column to what the surface actually sends. Found by sweeping the **third column**
  of the contract's minimum-code table — the one column no test had ever read.
  - **The defect.** Every run transition supplies the version it read, so a concurrent advance is
    refused by the adapter with `RepositoryError::VersionConflict`. That arrived at the API wrapped in
    `RunServiceError::Storage(_)`, so `service_error_response` sent it to **`500`** with the code
    `storage.version_conflict` — and `Retryable: false`. The contract lists `resource.version_conflict`
    for a precondition (`409`) and `internal.failure` for a `500`, so a client was told the server had
    faulted, under a code this surface does not document, when its own view was merely stale — the one
    case where a retry after a re-read succeeds. **The identical conflict on the policy route was
    already a `409`**, which is what made it a contradiction rather than merely a rough edge: one
    concept, two answers, depending on which route met it.
  - Fix: a `RunServiceError::Conflict` variant (a variant rather than a mapping tweak, because the
    *status* differs and the surface must not pattern-match a nested storage error to find that out),
    `From<RepositoryError>` mapping `VersionConflict` to it, and a `409` arm. Code, message, and
    retryable all become the policy surface's.
  - **A flag that had to stay different, and the reason is worth keeping.** `RepositoryError::retryable`
    returns **false** for a version conflict, with a doc comment explaining that a retry is "only
    meaningful with new information, so it is reported as **not** blindly retryable" — while
    `PolicyServiceError::VersionConflict` returns **true**. These are not in conflict: they answer
    different questions (the repository's "may this be resent unchanged?" versus the client's "may I
    retry after refreshing?"). My first framing treated them as contradictory and was wrong; the
    assertion now pins both answers *and* the fact that they differ by design.
  - **`the_tables_retryable_column_is_what_the_surface_actually_sends`** asserts the column against the
    real answers: `resource.version_conflict` is `yes` and the surface's own `Conflict.retryable()` is
    true, `service.not_ready` is `yes`, and the non-retryable rows are checked so an edit that flipped
    one without touching the table fails in the build rather than at a client. **Falsified:** flipping
    the table cell to `no` fails with "the contract marks a stale precondition retryable after a
    re-read"; and each half of the fix is falsified separately — sending `Conflict` back to `500`
    fails the status assertion, and collapsing the `From` arm back into `Storage` fails with "the
    adapter's version conflict must map to the conflict variant", which a test constructing the
    variant directly could not have caught.
  - **A second finding, recorded rather than papered over.** The table test's scan comment claimed the
    namespace list includes "`model.` and `run.`, which reach the envelope through the service error
    types this surface maps" — **neither is on the list.** `RunServiceError::Controller(e) => e.code()`
    yields `run.*` and `Self::Storage(e) => e.code()` yields `storage.*`, so those families reach the
    envelope while being invisible to a source scan here. The comment now says what the list is (the
    literals written *in* this surface, nothing more), the contract states the limit of the
    completeness check, and the scan and the retryable-column reader are extracted helpers so the test
    reads as an assertion rather than as a scanner.
  1003 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-042` Record the provider's own request id on the call, which the schema document claimed
  was read back but which no layer ever wrote. Found by following `provider_request_id` from the
  schema into the code — the sweep that found `BRN-017`'s `finish_reason` and `BRN-012`'s
  `error_code`.
  - **The defect.** `model_calls.provider_request_id` is a schema column, a line in
    `docs/data/schema.md`, a typed field on the port's `ModelCallOutcome`, an adapter bind, and part
    of the port's `StoredModelCall` read — and the controller wrote `None` on **every** path. The
    provider's id reaches a frame's `provider_metadata` (the scripted provider echoes the
    server-derived request id onto its start frame, exactly what `the_trusted_request_identity_reaches_the_adapter`
    asserts), and the controller's frame fold, `capture`, ignored `provider_metadata` entirely: its
    only argument was the frame's **kind**, so metadata attached to a frame had nowhere to go. The
    document's own sentence — "both are read back by the same port that writes them" — was true of
    the *read* and false of the *write*, which is the shape that hides: the round trip looked
    complete because every layer but the producer had the field.
  - Fix: `DrainedTurn` and `RecordedOutcome` gained `provider_request_id`; `capture` now takes the
    whole `ModelStreamEvent` (not its `&ModelStreamEventKind`) and reads `provider_metadata.request_id`
    off any frame; it is written with the outcome on every **non-refused** terminal path — the
    completion, the post-output cancellation, and the tool-fabric refusal — because those are the
    paths on which a provider accepted the call, which is what the identifier names. A refusal
    *before* acceptance (`fail_open`/`fail_after_acceptance`/`finish_expired`) keeps the plain
    recorder: no provider id was ever reported, so recording one would invent it.
  - **The residual is named, not wired.** `continuation_ref` is still passed as `None`: nothing
    resumes a provider call, so a stored value would be a column no operation reads — the
    permissive direction of the very defect this fixes. Recorded in `model-stream.md`.
  - **Falsified in both halves, each compiling:** making `record_call_outcome_with` send `None`
    fails `the_providers_request_id_is_recorded_on_the_call` with `left: None`; emptying the
    `capture` body of its `provider_request_id` assignment fails the same test the same way, which
    is what proves the fold, and not only the write, is on the path. The adapter's own round trip
    was already held by `a_model_call_attempt_records_and_loads`, so the missing producer was the
    only gap. And the value is verified end to end: `tests/e2e/policy-surface.mjs` now waits for the
    run to settle and reads `model_calls.provider_request_id` out of the profile's database, where a
    real daemon records the provider id — the assertion an API check cannot make, because no route
    exposes a model call.
  - **Two stale claims found while sweeping, and corrected in the same change.** `docs/data/schema.md`'s
    "Not done" paragraph said `model_route_decisions`, `model_data_policies`, and
    `model_policy_exceptions` "are not created, because routing and the data policy are `BRN-010`" —
    all three have been created (`000004`, `000005`) and written since `BRN-010` was implemented, and
    the paragraph named `BRN-005` (the run *state machine*) as the owner of the `agent_steps` port
    that never existed. And `MAX_EXCEPTION_REASON_BYTES` was declared **twice** — enforced in
    `jarvis_domain::model::exception`, and duplicated in `jarvis_application::repository::policy`
    where nothing referenced it — so the app copy is deleted and the domain's single definition
    stands.
  1004 workspace tests (+1). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-043` Produce the time-to-first-token instant the controller recorded but never measured,
  which was the last "wired but with no producer" column. Found by following `first_output_at` from
  the port into the controller — the sweep that found `BRN-017`'s `finish_reason` and `BRN-042`'s
  `provider_request_id`.
  - **The defect.** `ModelCallOutcome::first_output_at` existed on the application port, was bound
    by the SQLite adapter's `UPDATE`, was parsed by the adapter's `SELECT`, and was projected by the
    in-memory double — while **every controller path passed `None`**. So `model_calls.first_output_at`
    was `NULL` on every row, and the roadmap deliverable "measured incremental-delivery capability
    per model (time to first token and token spread)" had nothing to measure. `BRN-017` had named
    this precisely: "a wired column and an absent producer rather than a dressed-up feature", and
    left it to `BRN-011`. It is the same class as `finish_reason` (a value no layer produced) and the
    inverse of `BRN-012` (a column never written), and it is the last instance of the class because
    it is the last field the port carries with no producer.
  - Fix: `DrainedTurn` gained `first_output_at`, set with `get_or_insert` on the first
    `output.text.delta` — read from the **same** `occurred_at` the durable `run.output_text.delta`
    event carries, so the row and the event cannot name different instants for one token. It is
    written with the outcome on every path that produced output, including the ones that then
    **failed**: the post-output cancellation, the tool-fabric refusal, a mid-stream provider failure
    (`fail_after_acceptance`), the consumption-ceiling failure, and a mid-stream deadline
    (`finish_expired`). A call that emitted three tokens and then timed out produced a real
    first-token interval, and recording `None` for it would say no output arrived. The
    pre-acceptance paths (a refused `open`, an elapsed backoff) pass `None` explicitly, because
    there the column is genuinely empty — and the explicit `None` at those two sites is what makes
    the distinction visible rather than incidental.
  - **The instant is JARVIS's, not the provider's.** It is observed from the controller's own clock
    at the moment the first output is *seen here*, which is what an interval is measured against; a
    provider-supplied stamp would describe its side of a network JARVIS does not control, so two
    providers' numbers would not be comparable.
  - **Falsified in both halves, each compiling:** replacing `get_or_insert` with a plain assignment
    (last-wins) fails with `left: 2026-09-22T12:00:00.5Z, right: 2026-09-22T12:00:00Z` — a clock
    that jumps **between** two deltas distinguishes "first" from "last", which a fixed clock cannot;
    and shadowing the value back to `None` fails the same test, proving the producer and not only the
    storage. The test asserts the stored value read back through the port, not the argument the
    controller passed, because the port field and the bind both existed already. And the value is
    verified end to end: `tests/e2e/policy-surface.mjs` reads `model_calls.first_output_at` out of the
    profile's database after the run settles, where a real daemon records it — the assertion no API
    route can make, because none exposes a model call.
  - **What this does and does not close.** It produces the *input* `BRN-011` needs. It does **not**
    measure chunk spread, does not aggregate a capability per model, and does not enforce anything on
    a route — so `BRN-011` stays open, and the roadmap's "measured … per model" is nearer but not
    done. Recorded rather than implied, because a produced instant reads like the measurement it is
    only the first half of.
  1006 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-044` Stop sending the model its own question twice, which was a silent prompt defect rather
  than a missing feature. Found by reading the objective's path from `RunService::create` into the
  assembler and asking what the provider actually receives.
  - **The defect.** `RunService::create` appends the objective as the conversation's first user turn
    (`append_objective`) so the durable transcript holds the question, and passes the same text to
    `RunController::execute` as the run's task. The controller then assembles the transcript **and**
    an `ActiveTask` objective candidate — so the same question was a candidate twice and reached the
    provider **twice**. Nothing failed on it, which is what made it silent: the prompt was doubled, a
    long objective could push real conversation out of the budget, and the same tokens were billed
    twice against the ceiling. It is the inverse of the empty-objective defect round 21 found (that
    one sent nothing; this one sent everything twice), and like it the type system was satisfied.
  - Fix, in two halves, because either alone is a correct component with a broken product. The
    assembler now drops the transcript copy, and the removal is **by identity**: `append_objective`
    returns the `MessageId` it stored, `spawn_run` threads it to `execute`, which passes it through
    `build_context` to `context_assembly::assemble`, which skips exactly that message. Text matching
    was the first attempt and was rejected on a real case — a conversation where two runs asked the
    **same** question would have had the wrong turn dropped, turning a silent duplication into a
    silent **context loss**. The signature change carries an `Option<MessageId>`, where `None` means
    the objective was never stored as a message (the controller can be driven from a seeded
    transcript) and then nothing is removed.
  - **Falsified at both sites, each compiling.** Removing the skip in `assemble` fails
    `the_objective_is_sent_to_the_provider_only_once` with `left: 2, right: 1`. Passing `None` at the
    composition root fails
    `a_created_run_drops_the_objectives_stored_message_when_it_builds_the_prompt` the same way —
    which is the half a component test cannot see: the assembler stays correct and the daemon still
    sends the question twice. That second test drives the real `create` path and the real spawned
    task through a recording provider, and also asserts the objective **remains** a durable
    conversation turn, so "removed from the prompt" did not become "removed from the record".
  - A third test pins the identity rule directly: two messages with the **same text**, where the
    earlier one must survive. A content-matching implementation drops the wrong one and fails.
  - `create` crossed `clippy::too_many_lines` (104/100) with the added call, fixed by extracting
    `replayed_run`, which removes a genuinely duplicated five-field `CreatedRun` construction from
    the two replay sites — the extraction clippy was pointing at.
  1009 workspace tests (+3). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-045` Stop reporting a run `Completed` when its answer was never stored. Found by reading
  `complete_run`'s order and asking what a client sees when the step after the transition fails.
  - **The defect (an ordering defect, invisible from either operation alone).** `complete_run` moved
    the run `Responding -> Completed` and *then* stored the answer as the assistant's message. Both
    operations are correct on their own; the **order** is not. `store_answer` can fail on a real
    input — a provider that emits more than `MAX_CONTENT_BYTES` produces a content value the message
    port refuses — and when it did, the run was **already durably `Completed`**: its `run.completed`
    event claimed a delivered answer, the transcript held no assistant turn, and
    `GET /api/v1/runs/{id}` reported a successful run whose answer existed nowhere. The caller got an
    error while the durable record said success, so a polling client and a streaming client were told
    opposite things about the same run.
  - Fix: persist the answer **before** the transition, so "the run is completed" means "its answer is
    durable" — the durable transition is what a client reads. A storage failure now fails the run from
    `Responding` with `answer_not_stored`, using an edge the diagram already carries (added in
    `BRN-005`'s follow-up for the provider-failure-during-the-answer case), so the run neither claims
    success nor is abandoned mid-state. This is the storage architecture's own rule — persist the fact
    before publishing the claim — applied to the answer the completion claims.
  - **Falsified (compiling):** restoring the old order fails
    `a_run_whose_answer_cannot_be_stored_is_not_left_reported_as_completed` with
    `left: Completed, right: Failed`. The test drives a genuinely oversized provider output rather
    than a mocking of the port, so the failure is one a real provider can cause, and it asserts three
    things: the run is not `Completed`, its stored `error_code` equals the code its caller received,
    and **no assistant turn exists** for a run that delivered nothing.
  - The lesson is the one this project keeps re-learning from the other direction: the previous three
    rounds were about a value nobody produced, and this one is about a **sequencing** guarantee
    between two operations that each looked right. "Persist before publish" is stated for state and
    events; it is equally a rule for the answer the state claims.
  1010 workspace tests (+1). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-046` Stop recording a provider's own terminal as a successful run. Found by asking what
  the third terminal the contract names does to a run, after `BRN-017` and `BRN-042` had each taught
  that a frame field is only real once something reads it.
  - **The defect (a reader with no writer, twice over).** `docs/contracts/model-stream.md` says an
    adapter emits exactly one of `call.completed`, `call.failed`, or `call.cancelled`. The controller's
    frame fold (`capture`) had arms for output, tool calls, usage, and `call.completed` — and **no arm
    for either non-answer terminal**. So `call.failed` and `call.cancelled` left
    `drained.finish_reason` as `None`, and `finish_attempt` never read the finish reason anyway: it
    judged the stream by tool intent, ceilings, and completion only. A provider that ended its stream
    with `call.failed` produced a run `state: "completed"` whose stored answer was the very text the
    call had abandoned, with the call recorded `Completed` and the usage published — three durable
    records agreeing on a success that never happened. This is the same class as `BRN-042`
    (`provider_request_id` had no producer) and `BRN-017` (`finish_reason` had a writer and no
    reader), which is why the standing sweep now greps every contract-named terminal back to an arm
    that handles it.
  - Fix, in two parts. `capture` maps `call.failed` → `FinishReason::ProviderError` and
    `call.cancelled` → `FinishReason::Cancelled`, so the terminal is no longer discarded; and
    `finish_attempt` calls a new `settle_provider_terminal` **before** the completion path records the
    call, publishes usage, or stores the answer, because every one of those treats the stream's
    terminal as an answer. The two settle differently on purpose: `call.failed` fails the run and is
    never retried (the post-acceptance rule `fail_after_acceptance` encodes), while `call.cancelled`
    cancels it — the contract maps three terminals onto three outcomes, and reporting a provider's own
    cancellation as a fault would make it indistinguishable from a genuine failure. The call's outcome
    is closed with the same terminal the run got, so the row and the run cannot disagree about the one
    fact both exist to state.
  - **Falsified (compiling), one arm at a time.** Removing the `call.failed` capture arm fails
    `a_call_that_ends_failed_does_not_complete_the_run` with
    `Ok(RunOutcome { state: Completed, answer: Some("partial"), .. })`. Removing the `call.cancelled`
    arm fails `a_call_the_provider_reports_cancelled_does_not_complete_the_run` the same way. The two
    tests are deliberately not interchangeable: the cancelled test asserts the **exact** settled state
    (`RunState::Cancelled`) rather than "the run returned an error", so it also detects collapsing the
    two arms into one — the failed run's own test does not, which a mutation confirmed before the
    table row was written.
  - Extracting the settle into `settle_provider_terminal` also kept `finish_attempt` under the
    `too_many_lines` budget, which is what surfaced the size of what the completion path was doing
    unconditionally.
  1012 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-047` Stop storing a provider's refusal as an ordinary finish. Found by asking what the
  `refused` flag on `call.completed` does, after `BRN-042`/`BRN-043`/`BRN-046` had each taught that a
  field is only real once something reads it — and this one had a producer and no **reader**.
  - **The defect (a value carried and discarded).** The contract says `call.completed` includes
    "finish reason … and safety/refusal metadata", and the `refused` flag exists because a provider
    reports a **declined request** as an ordinary completion: the reason is `stop` and the refusal
    lives only in the flag. The controller's `capture` folded the reason and dropped the flag, so
    `FinishReason::Refusal` had **no producer anywhere in the workspace** and a refused answer was
    recorded exactly like a finished one. Two consequences, one visible and one latent: a caller
    reading the call could not tell "the model answered" from "the model declined", and the refusal
    count the observability architecture requires under *Models and Runtimes* could not be computed
    from any stored value, because the fact was gone before it reached the database. `capture`'s own
    doc comment claimed the opposite — "the flag and the reason agree here rather than becoming two
    conflicting spellings of one fact" — which is false for exactly the provider shape the flag was
    added for, and is the kind of claim no gate checks.
  - Fix: `completed_reason(provider_reason, refused)` resolves the two into one finish reason, and it
    **upgrades a plain `Stop` and nothing else**. A provider that names `ContentFilter` or `Length`
    has already made a more specific statement than "the model refused" — a provider-side filter
    stopping the output is a different fact from the model choosing to decline — so overwriting those
    would trade one discarded fact for another. `FinishReason::ContentFilter` is now produced through
    that same fold, which is the first producer it has had.
  - **Falsified in both directions, each compiling.** Ignoring the flag leaves the stored reason
    `Some(Stop)` where `Some(Refusal)` is required. Letting the flag overwrite unconditionally records
    `Refusal` for a `ContentFilter` stop the provider named. The refusal test scripts
    `refused: true` **beside** `FinishReason::Stop` rather than scripting `FinishReason::Refusal`,
    because the latter would assert only that the controller records the reason it was handed — a
    weaker claim that passes while the flag is discarded.
  - **Not done, and named rather than implied:** the refusal is recorded but nothing *acts* on it —
    `model-gateway.md` says a content refusal must not be retried, which the post-acceptance rule
    already enforces, but no metric or event counts refusals yet. That is the observability half
    (`OBS-*`), and the input now exists.
  1014 workspace tests (+2). Both doc gates green; all three journeys pass. **DO NOT COMMIT.**
- [x] `BRN-049` Make every validated newtype enforce its own rules **on the way in**, because a
  derived `Deserialize` bypasses the constructor and the invariant was only true for values this
  crate built. Found by a **systematic audit of every `#[serde(transparent)]` newtype**, prompted by
  one instance fixed during the filesystem round.
  - **The defect.** `#[serde(transparent)]` on a validated newtype derives a deserializer that calls
    `String::deserialize` (or `Vec::deserialize`) and wraps the result **directly**, never going
    through `Self::new`. So the byte bound, the NUL rule, and the non-emptiness invariant held for
    values the crate constructed and were **bypassed for values that arrived over the wire** — which
    is the direction an attacker chooses. Every downstream caller assumed otherwise, and one of them
    says so in code: `authorize_path` **skips** the shape check for a `WorkspaceRelativePath` because
    it believes the value is already normalized.
  - **Nine instances, four of them written before the audit existed.** `Scope`,
    `ServerConfigId`, `ListVersion`, `ToolArguments`, `ResultPayload`, `AllowedChannels`,
    `JsonText`, `TransitionReason`, and `WorkspaceRelativePath`. The consequence differed by type,
    and the worst was `AllowedChannels`: the constructor refuses an empty channel set to make
    "an approval that can never be decided" unrepresentable — and the derived deserializer restored
    precisely that state for any value arriving from storage or a client.
  - **Each is now hand-written to go through its constructor**, and
    `wire_validation_tests` asserts the shape that was missing for every one of them: the valid value
    round-trips, and **the value the constructor refuses is refused on the way in**. A test that only
    sent valid values in is what let this survive, since the constructor's own tests exercised the
    only path that was correct.
  - **Falsified two ways**, both by replacing a deserializer's validation with a direct wrap: the
    `Scope` case fails on a control character, and the `AllowedChannels` case fails on `[]` with the
    unreachable state restored.
  - **`ToolArguments` and `ResultPayload` are asserted to keep *different* bounds on the wire**, since
    those types exist because one bound had silently governed both before. A single shared
    deserializer would have been that same defect at the boundary.
  - 8 wire-validation tests. **DO NOT COMMIT.**
  **The sweep is now complete, and the result is that the defect was confined to `jarvis-domain`.** The
  other three crates carry exactly **one** `#[serde(transparent)]` newtype between them —
  `CredentialVerifier` in `jarvis-infrastructure` — and it has **no invariant to bypass**: it is a
  `[u8; 32]` whose constructor is infallible (`from_bytes` accepts any 32 bytes), so a derived
  deserializer is correct for it and a hand-written one would add nothing. That is worth recording
  rather than leaving as "not swept", because the absence of an invariant is the reason it is safe, not
  an oversight — and it is the distinction that decides whether a newtype needs a custom deserializer at
  all: **a bound, a non-emptiness rule, or a canonical-form rule needs one; a fixed-width value with no
  rule does not.**
  **⚠ That "complete" was about transparent newtypes only, and `BRN-060` found the same defect by two
  other routes** — a struct with public fields (`PreviewItem`) and a bare `String` field whose rule lived
  in a free function (`summary`). A sweep scoped to the *mechanism* it first observed cannot see an
  instance reached differently, so "the defect was confined to `jarvis-domain`" was true of the attribute
  and false of the class. `BRN-061` then widened the sweep to **every type with a `Result<Self>`
  constructor** and found nine more, three of which are correctly left derived because their only read
  path validates — so the rule is "a validating constructor **and** an unvalidated path to it", not
  "a validating constructor".
- [x] `BRN-048` Make code-page round-trip damage in a text file fail a gate, because it is
  invisible to every one of them while it sits in a committed file. Found by **scanning the bytes**
  of every tracked text file for the `C3 A2` sequence the earlier rounds had recorded as a hazard,
  rather than by reading code: `crates/jarvis-protocol/src/run.rs` carried **eleven** corrupted em
  dashes in its doc comments, introduced by commit `5bb0493` and present at `HEAD`.
  - **The damage is invisible by construction.** A file read with a legacy code page and written
    back as UTF-8 turns `E2 80 94` into `C3 A2 E2 82 AC E2 80 9D`, and the result still compiles,
    still resolves its doc links, still links from the index, and still passes `cargo fmt`. The
    defect was in the one file whose whole purpose is being read by a human, and nothing in the
    repository could see it — which is exactly the "a check nobody runs is not a check" class.
  - Fix, in two halves. The file is repaired by replacing the three-character sequence with a real
    em dash and writing it back as **UTF-8 without a BOM**, so the diff is ten lines and no other
    byte moves. Then `validateTextEncoding` in `scripts/validate-docs.mjs` reads every `.md`,
    `.rs`, `.toml`, `.json`, `.mjs`, `.js`, `.yml`, `.yaml`, and `.sh` file the walk reaches and
    refuses the artefact, reporting the file and **line**. It lives in the docs gate because that
    is the one command every change already runs.
  - **The guard found real instances the moment it was wired in, including in its own source.**
    The first run failed with 24 errors: my own `.scratch/` copies of the corrupted blob, and the
    literal damaged characters I had written into the checker's explanatory comment. The markers
    are now `\u` **escapes** — a literal copy of the damaged text in the guard would be the defect
    rather than the fix — and the scratch copies were deleted.
  - **Falsified both ways, and the second one is why the guard covers source.** A damaged `.md`
    fixture makes the validator exit `1`; so does a damaged `.rs` fixture, which the pre-fix
    validator passed. The source half is asserted separately because the file that motivated the
    guard was Rust, not Markdown.
  - **A second, pre-existing defect fell out of wiring the check.** Adding `.scratch` to the
    walk's skipped set dropped the validated Markdown count from **90 to 89**, because a stale
    `falsify43.md` left there since an earlier round had been counted as a document — so the
    gate's own headline number had never described the repository. A gitignored working area must
    be invisible here exactly as `target/` and `node_modules/` are, and that is doubly true once
    the check reaches `.rs` and `.mjs`, since a scratch copy of the corrupted file is precisely
    what a developer debugging it leaves behind. A fail-closed test now writes a `.scratch` probe,
    requires the run to pass, and asserts the count is **unchanged** — the count half is the
    stronger one, since a file that was counted while still passing is exactly how this hid.
  1014 workspace tests (unchanged); both doc gates green (18 fail-closed tests, +2); all five
  journeys pass (clean-machine, install, release, disconnect, policy-surface). **DO NOT COMMIT.**
- [x] `BRN-050` Make the run's retry policy settable on a request, because it was implemented and
  **unreachable**: `RunBudget::with_retry` had no caller outside the tests, so every run created
  through the daemon recorded `max_attempts: 1` and `RetryDecision::decide` answered `DoNotRetry`
  for every failure.
  Evidence: `BRN-008` built `RetryPolicy`, the pure `decide` function, the `FailureSite` boundary,
  the retry-chain storage (`logical_call_id` + `attempt`), and a suite of tests proving a transient
  pre-acceptance failure leaves the run live for a second attempt. **None of it could fire from a
  real request.** `grep 'with_retry'` over `crates/` returned five hits, all in test files, and
  `budget_for` — the one place a created run's budget is built — called
  `RunBudget::expiring_after` and folded the policy and route in but never a retry policy. The
  tests passed because they built their budget directly; a test that constructs its own input
  cannot discover that no request produces that input. This is the same class as a doc comment
  naming a function that does not exist, one level up: the capability was real, documented, tested,
  and unreachable.
  - **The wire field is optional and is never defaulted** (`CreateRunRequest.retry`, a
    `deny_unknown_fields` `RetryRequest` of three numbers). **Absent is not "no retries"**: an
    omitted field means the caller left the decision to JARVIS, while `max_attempts: 1` is a caller
    who has decided. A field with `#[serde(default)]` pointing at `RetryPolicy::none()` would
    collapse the two into one request, and the daemon would then be unable to distinguish a client
    that disabled retries from one that did not care — so the type keeps them apart and the
    *service* supplies the default. Asserted both ways by
    `a_callers_retry_policy_is_recorded_and_explicitly_disabling_it_is_honoured`.
  - **The daemon's default is bounded, named, and published.**
    `jarvis_domain::run::retry::RetryPolicy::default_for_run` is **3 attempts, 250ms base, 2s
    ceiling**; the attempt count is mirrored on the wire as
    `jarvis_protocol::run::DEFAULT_RETRY_MAX_ATTEMPTS` so a client can size its waiting and
    reconciliation window from the number the daemon will actually use. The two are compared in
    `jarvis-infrastructure` — the only crate that depends on both, since the documented flow is
    `Protocol --> Domain` — by `the_advertised_default_retry_attempts_is_the_one_the_daemon_applies`,
    which also asserts the default **permits a retry**, because a default of one attempt would make
    the published field meaningless and reintroduce the same unreachability.
  - **Out-of-range is refused, not clamped**, at the trust boundary with
    `request.semantic_invalid`. A clamped attempt count or a base above its ceiling is a policy the
    caller did not ask for and cannot detect — and it would size its own window against the number
    it sent. `parse_retry_policy` maps each `RetryError` variant to a message naming the bound that
    moved, because an operator's next step differs by which one it was.
  - **Falsified in both directions.** Ignoring the caller's policy (`with_retry(default_for_run())`)
    fails `a_callers_retry_policy_is_recorded...` with `left: 3 attempts, right: 2`; ignoring the
    default (`with_retry(none())`) fails
    `a_created_run_carries_a_retry_policy_a_transient_failure_can_use` and the pre-existing
    deadline test with `left: max_attempts 1, right: 3`. The second confirms the defect was real:
    the old code took the `none()` branch for every run.
  - **The retry safety boundary is untouched.** `FailureSite` is still a required input, the
    ambiguity rule is still checked first, and an accepted request is still never retried — this
    change only lets a request *reach* the policy that was already enforcing those rules.
  - 46 run-service tests (+2), 3 protocol wire tests (+3), 1 cross-crate agreement test (+2), 106
    HTTP tests (+1). Docs synced: `local-control-api.md` names the field and the absent-versus-
    `1` distinction, and `model-gateway.md`'s "not settable per request" claim is replaced by the
    section explaining what was unreachable and why.
  - **Still not done:** **fallback** (needs a capability inventory and candidate routes), the
    disconnect case, and a turn/byte/concurrency budget. A caller still cannot set the retry policy
    on an *existing* run, only at creation.
- [x] `BRN-051` Make the run's **step timeout** reachable, and record why the output ceiling's
  counterpart is deliberately left alone. Found by sweeping every `with_*` setter on a value type a
  request builds and counting **production** callers, the technique `BRN-050` produced.
  Evidence: the sweep listed eight domain setters; `with_context_tokens` had 0 production callers,
  `with_step_timeout` had 0, and `with_correlation`/`with_deadline`/`with_revision`/`with_route`/
  `with_policy` each had at least one. The **consumers** settled which mattered:
  `RunController::wait_bound` derives every provider await from
  `(remaining deadline, step_timeout)` and takes the tighter of the two, and `RunBudget::exceeded_by`
  compares reported usage against `max_output_tokens`. Both fields were read by real code and set by
  nothing.
  - **The step timeout was the load-bearing one.** `DEFAULT_STEP_TIMEOUT_MS` (two minutes) is now
    applied by `budget_for`, so a provider that accepts the connection and then stalls is cut off at
    two minutes instead of holding the run for the fifteen-minute default. It is deliberately
    **strictly below** `DEFAULT_RUN_BUDGET_MS`, because a step bound at or above the run's own
    deadline could never be the term the `min` picks — the same "a default that cannot bind is
    decorative" test the retry default carries.
  - **The output ceiling was implemented, then reverted, and that is the substantive finding.**
    Defaulting `max_output_tokens` makes `exceeded_by` fire and looks like the obvious parallel fix.
    It is wrong: the OpenAI-compatible adapter **does not forward `limits.max_output_tokens`** to the
    provider, so no provider is ever asked to be shorter — a defaulted ceiling would let a provider
    produce an over-long answer and then have JARVIS **discard** it, converting a working answer into
    a failed run while constraining nothing. The absent ceiling is the honest state until the adapter
    forwards the limit, and that is recorded in `model-gateway.md` rather than left as an omission.
    **A default is only correct when the thing it bounds can actually be told to comply.**
  - **Falsified:** removing `with_default_step_timeout()` fails
    `a_created_run_bounds_a_single_step_tighter_than_the_run_itself` ("**a single step must be
    bounded**") and the whole-budget assertion with `left: step_timeout_ms None, right: Some(120000)`.
    The output-ceiling default was falsified in the same way before being reverted, so the decision to
    drop it rests on a demonstrated mechanism rather than on a guess.
  - 47 run-service tests (+1, and the deadline test now compares the whole budget so a new default
    cannot be added without it failing). `local-control-api.md` and `model-gateway.md` synced.
  - **Still not done, each with its reason:** `max_output_tokens` reachable (needs the adapter to
    forward it — a two-part change), `with_context_tokens` never called (`max_context_tokens` is read
    with `DEFAULT_CONTEXT_TOKENS`, so this is a *configurability* gap rather than an unreachability
    one: absent and 8192 do the same thing), `max_cost_microunits` absent (there is no pricing
    catalog to bound against), `budget_is_verifiable` uncalled (the contract requires an unverified
    ceiling be *not reported as enforced*, which no surface does yet), and the context ceiling is not
    constrained by the routed model's attested window — `CapabilityDescriptor::max_context_tokens`
    still has neither producer nor consumer.
- [x] `BRN-052` Forward the run's output ceiling to the provider, so the ceiling is **enforced twice
  rather than neither way**. This is the missing half `BRN-051` identified and deliberately did not
  paper over: the previous round defaulted `max_output_tokens` was implemented, then **reverted**,
  because the adapter never sent the limit.
  Evidence: `RequestSchema` in the adapter's own evidence note listed only `model`, `messages`,
  `stream`, and `stream_options`, and `grep max_tokens` over the whole repository returned a single
  protocol **test fixture** and one `None`. So `RunBudget::max_output_tokens` was dead in both
  directions: no provider was told the bound, and `exceeded_by` had nothing to compare against on a
  real run. The distinction matters because the two halves fail differently — **a local check that
  only discards is not enforcement, it is data loss with a nicer name.** A provider that produces
  an over-long answer and has it thrown away is worse off than one never asked to produce it.
  - **The official reference settles the parameter name, and the obvious choice is wrong.**
    `max_completion_tokens` is "an upper bound for the number of tokens that can be generated for a
    completion, **including visible output tokens and reasoning tokens**". `max_tokens` is "now
    deprecated in favor of `max_completion_tokens`, and is **not compatible with o-series models**" —
    so a body using the older, more familiar name would be *refused* by exactly the reasoning models
    that most need an output bound. The adapter sends the current name and a test asserts the
    deprecated one is **absent**, not merely that the new one is present.
  - **The bound must measure the same thing JARVIS judges.** Because `max_completion_tokens`
    *includes* reasoning tokens, it bounds the quantity this adapter already maps from
    `completion_tokens` onto `Usage::output_tokens`, which is what `exceeded_by` compares. A bound
    over visible text alone would let the check fail an answer the provider considered within limit —
    a false failure, and the direction this project has recorded before as the worse one.
  - **An absent ceiling sends no parameter at all.** Not `null`, not `0`, not a default: `0` would
    ask the provider for an empty answer, and any invented value is a limit nobody set. Asserted
    explicitly, because "send nothing when unset" is the branch a later refactor drops.
  - With both halves in place, `budget_for` applies `DEFAULT_MAX_OUTPUT_TOKENS` (4096) — the default
    round 106 removed, restored now that it constrains something. `call_limits()` carries it to the
    request, so the field, the wire, and the check are one chain.
  - **Falsified in both halves.** Suppressing the forward (`&& false`) fails
    `the_runs_output_ceiling_is_forwarded_under_the_parameter_that_bounds_what_is_measured` with
    `left: Null, right: Number(1234)`; removing `with_default_output_tokens()` fails
    `a_created_run_bounds_its_output_and_the_breach_check_can_fire` and the whole-budget assertion
    with `left: max_output_tokens None, right: Some(4096)`.
  - **The research gate was honoured before the edit**: the note's own `llms.txt` root and API/reference
    indexes were used to locate `/api/reference/resources/chat.md`, and the quote above is from that
    page. The evidence note gains a `max_completion_tokens` mapping row, a "Data and Limits" subsection
    with the parameter decision and its citations, and a changelog entry. `Last verified`/`Revalidate
    by` are unchanged because the manifest pair must stay exact.
  - 1 adapter test (+1 with three cases), 47 run-service tests (+1), 1504 workspace tests.
  - **Still not done:** `max_cost_microunits` (no pricing catalog), `with_context_tokens` (a
    configurability gap: absent and 8192 behave alike), `budget_is_verifiable` (uncalled — nothing
    reports an unverified ceiling yet), and the context ceiling is still a fixed 8192 that ignores the
    routed model's attested window.
- [x] `BRN-053` Publish a run's **deadline** on the run resource, so the client-visible
  `run.deadline_exceeded` terminal is explicable to the client that receives it.
  Evidence: `RunController` returns `run.deadline_exceeded` from **three** paths — a deadline already
  past before any provider is contacted, a wait that outlives the remaining time, and a budget-caused
  retry refusal — and its own doc comment says a reader "knows to look at the configured budget".
  **No wire surface carried the budget.** `RunView` had `run_id`, `conversation_id`, `state`,
  `version`, `created_at`, `started_at`, `updated_at`, `completed_at`, and `error_code`, so a client
  that received the code could look at nothing: it could not tell how much time the run was given, how
  long remained, or whether a retry would fit inside the budget it was being refused by. The
  architecture's own durable-run-record list names "cancellation and **deadline state**", and only the
  cancellation half was ever visible.
  - **The value is the run's own stored column**, `agent_runs.deadline_at`, which `NewRun::with_budget`
    derived from `budget.deadline` — the same instant `RunBudget::status_at` compares against. So the
    published value is the one enforced rather than a second computation that could drift from it, the
    same reasoning `BRN-011` used for delivery profiles.
  - **An absolute instant, never a remaining duration.** A duration would restart its own countdown on
    every read, so two `GET`s of one motionless run would report different values under the same field
    name. The test asserts the instant's shape *and* that no `remaining`-shaped field exists.
  - **Omitted only when a run has no deadline**, which this build does not produce: `budget_for` gives
    every created run one, and a test in `run_service` asserts it cannot be absent. An empty string or
    a `null` would make "no deadline" and "a deadline at the epoch" indistinguishable.
  - **Falsified over the router**: projecting `deadline_at: None` fails
    `the_run_resource_publishes_the_deadline_that_explains_a_timeout` with the actual response body —
    `{"run_id":…,"state":"received","version":1,"created_at":…,"updated_at":…}` — and no `deadline_at`
    at all. The check is at the router rather than on the view type on purpose: a field that reaches
    the struct and not the wire is exactly this class, and a type-level test constructs the struct
    directly so it is structurally blind to a dropped projection.
  - **The CLI needed no change and that is the design working**: `jarvis runs show` prints the daemon's
    own body rather than a re-derived summary, so a new field reaches the terminal without a second
    renderer that could disagree with the daemon.
  - 2 protocol tests (+1) and 640 HTTP tests (+1). `local-control-api.md` names the field on the run
    resource and in the read's field list.
  - **Still not done:** the run does not publish its **remaining token/cost headroom**, only the
    deadline; and the `context_manifest_id` column remains `MEM-008`'s, as its own comment records.
- [x] `BRN-054` Record the usage that breached a consumption ceiling, so
  `run.budget_output_tokens_exceeded` is explicable to the client that receives it. The same defect
  as `BRN-053`, one field over.
  Evidence: the ceiling check runs **before** the usage event is published — deliberately, because the
  run must be failed and its answer discarded — and the terminal it calls wrote
  `RecordedOutcome { delivery, ..RecordedOutcome::default() }`, clearing the usage on the way. So a run
  failed for consuming too many tokens stored a model call that consumed **none**, and a reader sent to
  `run.budget_output_tokens_exceeded` had no number to compare against the 2048 ceiling that refused it.
  **That path discards the answer, which makes the usage the only evidence of what the provider
  produced** — so the one scenario where the numbers matter most was the one that recorded nothing.
  - **`finish_expired` now takes the usage** and writes `usage: usage.cloned()`, so the state, the
    event, the reason, the code, and the recorded call outcome are set in one place and cannot disagree.
    The ceiling path passes `Some(reported)` — the very value `exceeded_by` judged.
  - **`finish_deadline_exceeded` deliberately does not take one, and the earlier version of this change
    was wrong to add it.** Both of its call sites are waits that produced no report: one is a provider
    that never opened its stream, the other a frame wait that elapsed. Neither folds a usage into its
    drain, so a parameter there could only ever be passed `None` — and offering it invites a caller to
    pass `usage_of(&drained)` over an **empty** drain, which fabricates a report for a call that made
    none. The absent value belongs to the type, not to two call sites that must remember it.
  - **Falsified**: passing `None` on the ceiling path fails
    `a_run_that_exceeds_its_output_token_ceiling_is_failed_and_its_answer_discarded` with "**the usage
    that breached the ceiling must be recorded on the failed call**". The assertion was added to the
    existing test rather than a new one, because the property belongs to the case that test already
    owns — a separate test would let the original drift while the new one stayed green.
  - 77 controller tests, 1506 workspace tests. `agent-runtime.md`'s budget section records the path and
    why the deadline's sites take no usage.
  - **Still not done:** the run publishes no remaining token/cost headroom, and
    `budget_is_verifiable` remains uncalled, so a ceiling the provider left unmeasurable is still not
    **reported** as unverifiable.
- [x] `BRN-055` Publish **every** usage counter on the `run.usage` event. The adapter parsed four
  counters, the call row stored four, and the public event carried two.
  Evidence: `jarvis_infrastructure`'s adapter reads `cached_input_tokens` from
  `prompt_tokens_details.cached_tokens` and `reasoning_tokens` from
  `completion_tokens_details.reasoning_tokens` — the latter from a field the real endpoint capture
  found and the documented schema does not list — and its own test asserts both land on the `Usage`.
  The call row keeps all four. But `usage_payload` pushed only `input_tokens` and `output_tokens`, so a
  client following a run saw the provider's report **truncated**, and specifically could not see the
  reasoning spend that a reasoning model charges for. `model-stream.md`'s usage example lists all four
  counters, and `local-control-api.md` says this event carries "the counters the provider **reported**"
  — a counter JARVIS parsed, stored, and withheld from a public event is the reader half of the defect
  class this project has recorded four times.
  - **The counter set is now one table** (`for (name, value) in [...]`) rather than four `if let`
    blocks, so the list is readable in one place and adding a counter to `Usage` has an obvious home.
  - **Asserted as a whole object, and that is the load-bearing part of the fix.** The previous test
    checked the two fields it expected to be present — and both passed, because the omission was of two
    fields nothing mentioned. Comparing the entire payload (minus the expected `call_id`) is what makes
    a *fifth* counter's omission fail here rather than pass silently again.
  - **A deliberate non-change: `provider_reported` is not published.** `model-stream.md`'s usage shape
    includes it, so adding it is tempting — but `grep 'provider_reported: true'` finds **no `false`
    producer anywhere**: the adapter sets `true`, and every other occurrence is a fixture. Publishing a
    constant boolean would put a field in a durable public payload that can never carry information,
    and the honest state is to leave it absent (as the contract already permits: "Missing usage fields
    remain `null`/absent") and add it when JARVIS has an estimator. The same reasoning applies to
    `estimated_cost_microunits` and `currency`, which have no pricing catalog.
  - **Falsified**: removing `reasoning_tokens` from the table fails
    `the_usage_event_carries_every_counter_the_provider_reported` with the payload as published —
    `{"call_id":…,"input_tokens":11,"output_tokens":22,"cached_input_tokens":7}` against the expected
    four-counter object.
  - 78 controller tests (+1), 641 HTTP tests (+1 guard), 1507 workspace tests. `local-control-api.md`
    names the four counters and records why the set is compared whole.
  - **Still not done:** on the `run.usage` *event* — which reports one call — `provider_reported`,
    `estimated_cost_microunits`, and `currency` remain unpublished because nothing can produce a value
    for them; and `budget_is_verifiable` remains uncalled. Usage **is** now summed across a run's calls
    for the run *read* (`BRN-058`), where the fold has a source: each call's stored block.
- [x] `BRN-011` Measure and record incremental-delivery capability per model
  (time to first token **and** chunk spread) rather than a streaming boolean, and
  fail a route selection when a pinned model reports streaming but delivers its
  output in one burst.
  Evidence: **both halves now exist** — the per-call measurement (previous round) and the per-model
  aggregation that was the recorded gap. `DeliverySamples` in
  `jarvis_domain::model::capability` is a *campaign* and `aggregate` folds it into one
  `IncrementalDelivery`; `ModelCallRepository::delivery_campaigns` reads the samples from
  `model_calls`; `route_candidates` attests them onto the descriptor the selector reads; and
  `RunPorts::delivery_campaigns` carries them from the composition into run creation.
  - **The gap, closed by connecting an existing producer to an existing consumer.** The previous
    round recorded three timings on every call that emitted output, and every
    `CapabilityDescriptor` in the workspace was still built with `incremental_delivery: None` in the
    one place (`run_service::route_candidates`) that constructs them. So the figures existed, the
    profile type existed, the routing predicate that consumes it existed, and **no route could ever
    require incremental delivery** because nothing ever attested it. This is the `BRN-017` / `BRN-042`
    / `BRN-043` class a third time: found by asking what consumes the value, not by reading the type.
  - **The previous round's half, preserved because it explains the columns this one reads.**
    `last_output_at` and `output_delta_count` were added in migration
    `000006_model_call_delivery_profile.sql` (schema version 6) because one instant and a first token
    cannot distinguish a stream from a burst. The profile is **derived, not stored**, so editing
    `MIN_INCREMENTAL_SPREAD_MS` applies to every row rather than only to new ones; a negative elapsed
    interval is clamped to zero rather than refused, because a subtraction is not where a completion
    path should fail; and the three timings travel as one `DeliveryTiming` value, since the two
    instants are the same type and transposing them would compute the interval backwards.
  - **Two defects the previous round's own tests found, both recorded rather than smoothed over.**
    The half-measurement was **storable** — `ModelCallOutcome`'s fields are public and `validated()`
    ran only in the constructor, so a caller could persist a spread with no first instant; both the
    adapter and the in-memory double now validate at the boundary. And a no-output call recorded
    `output_delta_count: Some(0)`, a *verdict* where the column documents `NULL` as "not measured" —
    **the round's own test asserted `Some(0)` and passed**, which is the round-17 lesson: a test can
    encode the bug it should catch. The count is now derived from the first-output instant, so the
    bad shape is unconstructible. ⚠ The falsification cycle also produced a **false result** worth
    remembering: restoring a mutated file with `Copy-Item` preserved the *backup's* older mtime, so
    Cargo reused the **mutant** binary and the test appeared to fail after restoration. A
    mutation/restore cycle must force a rebuild, or the restore is unverified.
  - **`000006` needed an upgrade test a fresh database could not be.** It is the first migration here
    that `ALTER TABLE ADD COLUMN`s a table which can already hold rows, so
    `a_fresh_database_migrates_and_reports_the_target_version` was the wrong evidence for the row
    `docs/data/migrations.md` heads "DB older, auto-migration safe" — and `AGENTS.md` requires
    "test migrations from supported prior versions". `a_database_from_a_supported_prior_version_
    upgrades_in_place` applies the first five migrations by hand, writes a conversation, run, and
    model call at version 5, upgrades with this binary's own migrator, and asserts the old row
    **survives with `NULL` for both new columns**. That last assertion is what makes it an upgrade
    test: a migration that backfilled `0` would claim every old call was measured and delivered
    nothing — a burst verdict invented for a call nobody measured. Falsified by adding that backfill.
  - **The aggregate is the worst sample, not the mean, and that is the design decision.** A campaign
    in which any call burst is a model that *can* burst, so `chunk_spread_ms` is the **minimum**
    observed, `observed_deltas` the minimum, and `time_to_first_token_ms` the **maximum**. A mean —
    the obvious implementation — reports "delivers incrementally" for a model that sometimes does not,
    and a caller relying on that waits for the whole generation on exactly the call that burst. The
    field is deliberately not named `average_spread_ms` so the mistake cannot be made by naming.
  - **`MIN_PROFILE_SAMPLES` (3) is a floor on sample size, not a confidence claim, and it is checked
    inside the attesting call.** "One call is an anecdote" is the whole difference between *measured*
    and *measured once*: a single attempt's figures describe that attempt — the prompt it happened to
    send, the load it happened to meet — so a descriptor built from one row would tell the router a
    model's *profile* while having observed one sample of it. Below the floor the descriptor keeps
    `None`, which makes a route requiring incremental delivery **refuse**; the floor is enforced in
    `attested_profile` rather than at a call site, so a caller cannot reach
    `Some(Attested { label: Verified })` with one sample however it builds the campaign.
  - **Measured evidence is `VERIFIED` and cites no document.** `VERIFIED` is defined as "confirmed by
    a current official specification, schema, or **live test against the pinned version**" — a
    recorded call against the configured endpoint is exactly that live test, and `DOCUMENTED` would be
    wrong in the other direction because no document states these figures. The `source_url` is
    `jarvis://model-calls`, a scheme with no host, rather than an empty string (reads as a defect) or
    a plausible-looking `https://` address (**cites a page that does not say what the value says** —
    the promotion the evidence rule exists to prevent). The revalidation window is derived from the
    measurement day via a new `IsoDate::adding_days`, so a profile cannot be created already expired
    or hand-dated into the far future, and the absence of a clock fails **closed**: the inventory
    stamps `IsoDate::UNIX_EPOCH`, which every freshness check reads as stale, rather than silently
    reading a real clock and defeating the reproducibility the parameter exists for.
  - **The read filters, and the filter's redundancy is recorded rather than implied.** Only calls that
    emitted output contribute, because a call with no first-output instant was never measured and
    folding it in as zero would report a burst for a model that was never allowed to stream. The three
    `IS NOT NULL` clauses are written separately as defence in depth against a row this binary did not
    write, and **removing one of them does not fail a test** — the other two exclude the same rows,
    because the write path always sets all three together. That was discovered by falsifying it, and
    is now stated in the code and the test rather than left as an implication: the falsification that
    means something is removing *all three*, which is what the test was rewritten to fail on.
  - **Falsified four times, and one attempt taught something.** (1) Changing the spread fold from
    `min` to `max` fails `one_burst_sample_makes_the_whole_campaign_a_burst` and
    `the_aggregate_takes_the_pessimistic_figure_for_every_field` with
    `assertion left == right failed: the narrowest spread is the one that decides` — the assertion
    the whole rule exists for, and the same fixture is asserted to be one the averaging rule would
    wrongly accept. (2) Removing **one** `IS NOT NULL` clause passes, which is the finding above.
    (3) Removing **all three** fails `an_unmeasured_call_contributes_no_delivery_sample`. (4) The
    domain test `one_single_delta_sample…` **failed on first run and exposed a wrong fixture of my
    own**: `campaign(&[…, (900, 1)])` passes `observed_deltas` directly, and a one-delta call derives
    `0` — so the fixture was constructing a profile `DeliveryMeasurement::profile` cannot produce.
    The helper's parameter is the *derived* figure, not the raw count, and the test now says so.
  - ⚠ **A PowerShell quoting trap that cost three attempts, worth recording.** Mutating a SQL string
    literal containing backslash line continuations via
    `$orig.Replace('…\' + "`n" + '…')` left PowerShell on a continuation prompt and needed Ctrl-C;
    the file was unharmed but the mutation never applied. A literal containing `\` + newline must be
    mutated from a script (`.scratch/mutate.mjs`, which reads with `readFileSync`, applies the
    replacement, runs the command, and **restores the original bytes**), not through PowerShell's
    quoting.
  1039 workspace tests (+15: domain 255 -> 261, application 215 -> 220, infrastructure 469 -> 473);
  `fmt` and `clippy -D warnings` clean; both doc gates green; all five journeys pass.
  **Not done, and named:** the pipeline is complete end to end but **no real provider has been
  measured**, so for a real model the item is still unmeasured — a route that requires incremental
  delivery is refused, correctly, because nothing real has produced figures. `model-gateway.md`'s
  list item therefore stays `UNVERIFIED` for a real model: the remaining work is a campaign against a
  real endpoint, which is `BRN-003`'s adapter plus operator time rather than more code. Retention and
  training-use evidence is still absent from `route_candidates` (`None`), which is the data-policy
  half and is `BRN-010`'s, not this item's. **DO NOT COMMIT.**
- [ ] `BRN-082` Implement auxiliary model routes: a named route for vision,
  summarization, classification, and the post-turn learning review; byte-identical
  prompt-cache parity when an auxiliary call resolves to the parent model; a digest
  replay when it does not; and a cumulative replayed-input-token budget that stops
  the call and reports the stop. Record auxiliary usage under its own task label so
  it is separable from conversation usage. See
  [model-gateway.md](docs/architecture/model-gateway.md) and
  [ADR-0012](docs/adr/0012-governed-learning-loop.md).

## Milestone 3: Tool Fabric

Dependencies: Milestone 2 exit gate.

- [x] `TLS-001` Define canonical tool schema, identity, origin, effects, scopes,
  risk, timeout, and retry metadata. Owns `jarvis_domain::tool`: the definition, its
  canonical identity, and the classification it declares. **A discovery defect in the
  error-code boundary was found and fixed while building this** — see below, because it is
  the more consequential of the two changes.
  Evidence: `identity.rs` holds `ToolCapability` (`namespace.name@major`),
  `ToolVersion`, `SourceKind`, `ToolSource`, and `SchemaFingerprint`.
  **`ToolIdentity` has no display-name field**, which is what makes the contract's
  "names are aliases" rule structural rather than documented: `ToolIdentity::authorizes`
  is the single place that decides whether a recorded grant still covers a tool, and
  `ACC-024` — *"replace an MCP/plugin tool schema/source behind the same display name;
  existing approval/grant cannot authorize the replacement"* — is now an executable test
  (`an_approval_for_a_tool_does_not_authorize_a_replacement_behind_the_same_name`) that
  asserts an unchanged display name and an unchanged capability while the source **and**
  the schema change, because a naive check comparing names or capabilities alone would
  pass exactly that scenario. Three further facts prevent the replacement:
  `SchemaFingerprint` requires the `sha256:` prefix rather than a bare digest, so a
  digest from another algorithm cannot compare equal; `SourceKind` distinguishes an MCP
  server from a native tool so two servers publishing one capability are different tools;
  and the major version is part of the capability, so `@2` is a *different* tool rather
  than a replacement of `@1` (`is_replacement_of` and `authorizes` are separate answers
  because "the tool you approved was replaced" is a different operator conversation from
  "that tool is not this one").
  **Four falsifications, each reverting one rule to its naive form.** Replacing
  `authorizes`' whole-tuple comparison with a capability-only comparison fails
  `the_same_name_from_a_different_source_is_not_the_same_tool` and
  `a_changed_schema_under_the_same_source_is_not_the_same_tool` — that is `ACC-024`
  failing exactly as written. Making `is_replacement_of` always `false` fails both.
  Allowing an empty effect list fails `a_definition_may_not_declare_no_effects`.
  Disabling the risk/approval rule fails
  `a_high_risk_tool_with_no_scopes_may_not_default_to_allow`.
  `classification.rs` holds the closed sets (`Effect`, `Risk`, `ApprovalHint`,
  `Idempotency`) and their parsers, each of which **refuses** an unknown value rather
  than defaulting: the fail-open default of each — an unknown effect ignored, `severe`
  read as `Low`, `maybe` read as `Allow`, `probably` read as `NaturallyIdempotent` (which
  would skip the reservation) — is the arm a catch-all would have written, and each is
  asserted by its own field name. `DataClasses` reuses the model gateway's `Sensitivity`
  rather than defining a second ladder, and refuses an output classified **below** its
  input: a result is derived from the arguments, so the only direction derivable from the
  input alone is upward, and allowing the other would let a tool launder a classification.
  `definition.rs` enforces the cross-field rules — a non-empty effect list, `read_only`
  not combined with another effect, a high-risk tool with no scopes not defaulting to
  `Allow`, and the capability's major matching the release's.
  - **⚠ The more consequential find: the error-code boundary recognized only the `jarvis.`
    prefix, and the contracts define codes under nine namespaces.** `ErrorCode::new` was
    covered by exactly one test, and that test used **`tool.permission_denied` — a code
    `tool-contract.md` defines — as its example of a value that must be REJECTED.** So the
    single test for the rule encoded the bug instead of catching it, and the consequence
    was not cosmetic: the tool contract's sixteen `tool.*` codes, the approval contract's
    twelve `approval.*` codes, and the `run.`, `stream.`, and `storage.` families would all
    have been rewritten to `jarvis.internal` at the boundary, so a client told to branch on
    `tool.rate_limited` or `tool.permission_denied` could no longer tell a rate limit from a
    rejection. The fix is a namespace **set** (membership, not a prefix test — any dotted
    string starts with something, so accepting everything would let a provider name a code
    JARVIS forwards as its own), requiring a separator and a non-empty segment so `toolbox.x`
    and the bare `tool.` are refused. Falsified three ways: reinstating the old
    prefix-only rule ahead of the set fails the namespace test, dropping the separator check
    fails on `toolbox.x`, and allowing a bare namespace fails on `tool.` — and the namespace
    test compares its sample table against `ErrorCode::NAMESPACES` as a **set in both
    directions**, so adding a namespace without proving it is reached cannot pass.
    `common-conventions.md` now states the nine namespaces normatively, so the rule that
    decides which codes may exist is visible where the contract is rather than only in the
    function.
  - 30 domain tests in `tool_tests.rs` plus 3 for the namespace rule. 1186 workspace tests.
    All gates green. **DO NOT COMMIT.**
  **Not done**, and deliberately not claimed: this is the *definition* only. There is no
  registry, no discovery, no validation of arguments against the schema, no policy
  evaluation, no approval record, no execution, and no call ledger — those are `TLS-002`
  through `TLS-006`, and `TLS-001`'s own text asks only for the metadata. The
  `input_schema`/`output_schema` documents are **not stored on the definition**: the
  contract lists them, and what exists is the *fingerprint* of the input schema, which is
  the part identity binds to. Holding the schema text needs a JSON implementation in the
  domain or an adapter that owns the schema subset, and neither exists yet, so the field
  would be a place to put a document nothing reads — the `model_calls` shape this project
  keeps finding. It is named here rather than added empty.
- [x] `TLS-002` Implement registry discovery independently from grants. Owns
  `jarvis_domain::tool::registry` and `jarvis_domain::tool::discovery`.
  Evidence: the word the TODO turns on is **independently**, and the architecture states both
  halves of why: *"Tool discovery never grants execution permission"* and *"Selection affects
  model context, not authorization at execution time"*. So `ToolRegistry` answers exactly one
  question — which definitions exist — and holds **no grants at all**, which is asserted as a
  structural test rather than a claim: the test reaches every type a caller can get from the
  registry and shows they carry classification (`effects`, `risk`) and no permission, because a
  registry that grew an `allowed: bool` would be a second authorization decision living beside
  the policy layer and the one consulted first would be the one that mattered.
  Two of the tool fabric's own required tests are now executable, and both are silent failures
  in a naive implementation:
  - **"tool-name and source-identity collision."** An identity names a *source* as its owner and
    version, and those come from the server's own manifest — **untrusted input**. Two servers
    both declaring `acme.files 1.0.0` produce the *same identity*, so the second inherits every
    approval recorded for the first. The trusted side of the check is therefore the **server
    configuration identity**, which is why the registry records it (and why it is a separate type
    from `ToolSource`: one is JARVIS-assigned and trusted, the other is a claim the tool makes).
    A second server claiming a known source is refused **unconditionally** — even with
    `RegistrationRequest::replacing`, because the collision is the thing a replacement flag would
    be used to authorize. `deregister` releases the claim when the source's **last** tool goes,
    and a test asserts both sides: removing one of two tools keeps the claim, since otherwise a
    rogue server could claim a source the first still offers a tool under.
  - **"stale discovery cache and source replacement."** The contract's cache key lists five
    scopes plus the list-result version, and `DiscoveryCacheKey` holds them together so a partial
    key is **unrepresentable** rather than remembered. `a_scope_differs_when_any_scope_component_differs`
    varies each of the five one at a time and requires a different key; `invalidate_server` drops
    every answer from a replaced server (the enumeration a caller would get wrong);
    `invalidate_below_version` sweeps a superseded list version per server; and staleness is
    **counted separately from a miss** because "no answer was ever computed" and "your answer is
    out of date" lead a caller to different next steps.
  - **The disclosure case the key exists for.** A local profile has ONE workspace shared by every
    client, each with its own principal, so a cache keyed on the server and workspace but not the
    principal answers one client's discovery call with another's catalog. That is the same
    three-of-five-dimensions shape `BRN-007`'s idempotency key had, and the test states it as a
    scenario (`an_answer_computed_for_one_principal_is_not_served_to_another`) rather than as an
    assertion about key equality.
  - **Four falsifications.** Making the source-claim check unreachable fails 2 tests; making
    freshness unenforced fails 3; making the existing-identity branch unreachable fails 4; and
    **dropping the principal from the discovery scope fails 5, including the disclosure test**.
    A fifth mutation (not storing a permitted replacement) did **not compile** and was redone in
    a form that did, because a mutation that fails to compile is not evidence.
  - A registration is **idempotent when nothing changed** (`Unchanged`, because a server re-lists
    its tools on every reconnect and refusing that would make a healthy reconnect look like an
    attack) and a **permitted change reports itself** (`Replaced`) so an adapter can log that an
    approval must be re-obtained rather than recording success while the implementation behind a
    live approval changed.
  - 24 registry tests. 1210 workspace tests. All gates green. **DO NOT COMMIT.**
  **Not done**, and named: nothing *calls* the registry yet — there is no MCP client, no adapter
  that computes a schema fingerprint, and no HTTP route, so the registry is a complete domain
  capability with no producer. `TLS-003` (argument validation against the schema) and `TLS-004`
  (policy) are the next consumers, and until one exists the registry is exercised only by its own
  tests. `DiscoveryScope` deliberately has **no `expires_at` field**: the TTL is applied when the
  answer is stored (`DISCOVERY_TTL_DAYS`) rather than carried in the key, because a key that
  varied with time would make every entry distinct and the cache useless.
- [x] `TLS-003` Implement input/output validation and bounded result storage. **Argument validation
  against a schema is now implemented, so all three words of the entry are done.**
  Evidence: `jarvis_domain::tool::call`, `jarvis_domain::tool::error_class`, and
  `jarvis_infrastructure::tool_schema`. Research evidence:
  [JSON Schema 2020-12 validation](docs/research/integrations/json-schema-validation.md).
  - **Bounded result storage is done, and building it found a real defect.** `ContentBlock`,
    `ResultPayload`, and `ToolResultBody` bound a result by **bytes** rather than characters (a
    character-counted bound admits four times the memory for the same number, and a test uses a
    multi-byte string to prove the difference is enforced). The defect: the first version held each
    block's payload in `ToolArguments`, which bounds at `MAX_ARGUMENT_BYTES` — the *argument* limit —
    so a block could never exceed 64 KiB and the larger `MAX_RESULT_BYTES` was **unreachable**. The
    two bounds differ on purpose and the code says why: an argument document is model output, while a
    result legitimately carries file contents and search hits. Two bounds need two types, or the
    stricter one silently governs both. `a_block_is_bounded_by_the_result_bound_rather_than_the_argument_bound`
    is the regression test, and reverting the payload to the argument bound fails it.
  - **The total is enforced in the constructor, not left to the caller.** The per-block bound cannot
    substitute: `MAX_RESULT_BLOCKS` blocks each at `MAX_RESULT_BYTES` is 64 times the intended total.
    A first version returned the oversized body and exposed `is_within_bound` as advice, so a result
    of 64 legal blocks constructed successfully — a bound whose invalid value can still be built is
    not a bound. Falsified by disabling it, which fails 3 tests.
  - **The error classes are a closed set of sixteen**, parsed only from the contract's codes, so a
    provider's error text must be *mapped* by its adapter rather than passed through. Each carries a
    **`RetryPosture` rather than a `retryable` boolean**, because this project's own rule is that
    "retryability describes a specific operation outcome, not an error class in every context" — and
    for a tool call the same class answers differently per tool. `Timeout` on a tool that declared
    `Idempotency::None` must not be retried (the message may already have been sent) while the same
    class on a caller-keyed tool may be. A per-class boolean would have to pick one answer and be
    wrong for the other, and the wrong direction **sends twice**. Both directions are falsified, since
    a test asserting only `true` would pass against an implementation that always said `true`.
  - `ToolCallIntent` holds **no principal, workspace, or grant** — the contract says those are
    "trusted context, not accepted from model arguments", and carrying them would move the trust
    boundary to whatever produced the intent, which is commonly a model. Its `capability` is a
    bounded `String` rather than a parsed `ToolCapability` **on purpose**: a model emits a name, and
    "the model named a tool that does not exist" has to be representable, because that is what
    produces `tool.not_found` rather than a parse refusal. Its `Display` renders the name and the
    argument *size* and never the argument bytes, asserted by searching the rendered form for a
    secret-looking payload.
  - **Also swept from round 84:** `ToolRegistry` held a `capability_index` that was **written and
    never read** — the "field with no reader" shape — whose comment claimed it made a capability
    collision detectable while nothing consulted it. Removed, and the removal is why the code now
    says explicitly that a shared capability is *not* a conflict (two majors, two servers) and that
    the source claim is the check that makes impersonation impossible.
  - 24 tests in `call_tests.rs`. 1234 workspace tests. All gates green. **DO NOT COMMIT.**
  - **Argument validation against a schema is done, and it is its own crate-boundary decision.**
    `jarvis_infrastructure::tool_schema::ToolSchema` parses a schema, walks it, refuses it if it
    cannot be honoured, and decides an argument document against it. It is in
    `jarvis-infrastructure` rather than the domain because the domain has **no JSON dependency at
    all** — a parsed schema and a parsed instance are both `serde_json::Value` — and because the
    supported-subset decision is an integration decision. That is the same split, for the same
    reason, as `tool_fingerprint` beside it.
  - **The subset is decided twice, and the second decision is the important one.** The keywords
    JARVIS implements are a table; everything else is **refused at load** rather than ignored. The
    specification says an unrecognised keyword "SHOULD be treated as an annotation" (Core §6.5), so
    the compliant-looking implementation is the one that silently validates *less* than the schema
    asked for. A refusal is an operator-visible configuration defect; an ignored assertion admits a
    malformed tool call. `pattern`/`patternProperties` are refused because evaluating them needs a
    reviewed regex engine, and the Validation specification's own Security Considerations names why
    ("catastrophic backtracking ... denial-of-service"); `multipleOf` is refused because its exact
    decimal semantics cannot be honoured by `f64`.
  - **Two resource bounds, each required by a specification, and the depth bound had to be
    MEASURED rather than chosen.** `MAX_SCHEMA_STEPS` is charged per keyword and per `uniqueItems`
    comparison, because a bounded input does not bound the work — a 633-item array exhausts it
    while fitting comfortably in an argument document. `MAX_SCHEMA_DEPTH` covers schema descent,
    instance descent, and `$ref` hops, since a `$ref` cycle consumes no instance. It is **32, not
    128**, because `serde_json` refuses a document nested more than 127 containers deep and a schema
    needs about two containers per level — a bound near 128 could never fire, so it would have been a
    declaration nothing enforced. The test asserts that ordering, so lowering it past the parser's
    limit fails in the build instead of silently making the bound dead.
  - **The specification's number equality rule is implemented, not `serde_json`'s.** Core §4.2.2 says
    two numbers are equal when they "have the same mathematical value" and that "trailing zeros" are
    insignificant, so `enum: [1.0]` must accept `1`. The derived comparison does not follow that
    rule, and a validator that used it would reject a document the schema accepts.
  - **`items` and `additionalProperties` read their SIBLINGS.** Core §10.3.1.2 defines `items` in
    terms of a sibling `prefixItems`'s length and §10.3.2.3 defines `additionalProperties` in terms
    of sibling `properties`, so both need the schema object rather than their own value. A test with
    only one of each pair present passes against the wrong implementation, so both are tested with
    the sibling absent *and* present.
  - **`ToolSchema::confirms` is what keeps this from being a component nothing consults.** A tool's
    identity is bound to a `SchemaFingerprint` and an approval is recorded against that identity, so
    the fingerprint a definition carries must describe the schema calls are actually validated
    against. Without the check a definition could state one schema's fingerprint while a looser
    schema was in force — `ACC-024`'s failure mode reached through two disagreeing values instead of
    through a display name. Proven end to end in
    `tool_schema/composition_tests.rs`, which is the only place that can compose a domain
    `ToolDefinition` with a parsed schema.
  - **Falsified 15 ways, zero survivors, and two harness defects were caught in the process.** The
    mutation harness's "did it compile" check matched cargo's `error: test failed` line, which read
    all 13 killed mutations as unproven; and one mutation — `additionalProperties`'s sibling read —
    was a **no-op**, because that arm is guarded and never executes. A mutation that cannot execute
    proves nothing, exactly like an unreached branch not counting as covered.
  - 34 tests (`tool_schema/tests.rs` 30, `tool_schema/composition_tests.rs` 4). 1557 workspace tests.
  - **Not done, and it is the half that makes this reachable: no adapter stores a schema and no
    request reaches the validator.** The validator is complete and composes with a definition, but
    the fabric as a whole has no HTTP surface, so every tool-fabric type is still reachable only from
    tests. `TLS-002`'s registry, `TLS-004`'s policy, `TLS-005`'s approvals, and `TLS-006`'s ledger
    are in the same position: complete domain capabilities whose first production consumer is the
    tool surface that does not exist yet. Nor is there durable result storage — these are in-memory
    values, and the ledger that would persist them is `TLS-006`.
- [x] `TLS-004` Implement deterministic policy evaluation and explainable decisions. Owns
  `jarvis_domain::tool::policy`.
  Evidence: `evaluate` is a **pure function** of a request and its inputs, and determinism is a
  property of the *types* rather than a promise in a comment: `PolicyInputs` holds values — a
  timestamp, grants, approvals, deny rules — and no port, clock, environment, or I/O, so the function
  cannot read one even by accident. `now` is a parameter exactly so two evaluations of one request are
  comparable, which is what an audit needs when it asks which decision authorized an effect. Two tests
  assert it: the same request evaluated twice is identical, and a request evaluated with its grant list
  in reverse order is identical — the second is the stronger claim, because a first-match-wins
  evaluator would answer differently for the same *set* of grants depending on how a repository
  happened to order them, and a repository order is not a policy.
  - **Precedence is the content of the module.** Deny rules are consulted **first**, because they are
    the only input with no override; a grant consulted first would let a grant override an explicit
    refusal. Availability and the run's deadline come next, then grant resolution, then grant
    constraints, then approvals, then the tool's own default. Falsified by disabling the deny-rule
    check, which fails 3 tests including `an_approval_can_never_overturn_a_deny`.
  - **No allow by omission.** Every `Allow` carries a positive reason — `ApprovalMatched`,
    `LowRiskReadOnly`, or a later layer's — and a request with no grant, no approval, and no deny rule
    is **denied**. That is the single assertion a happy-path suite omits, and it is falsified by
    substituting a fabricated permissive grant for the real ones: the test then fails alongside
    eleven others.
  - **Three refusals a user reacts to differently are kept distinct**: no grant, grant expired, and
    grant naming a **different implementation** of the same capability. The last is `ACC-024` at the
    policy layer — a grant exists and still names the tool, while the source or schema changed — and a
    user told "no grant" would go looking for a missing grant rather than at the replacement. The
    boundary is asserted too: a grant for a *different* capability is `NoGrant` rather than a
    replacement, because otherwise a user would be told their tool was replaced when they never had a
    grant for it.
  - **An approval can never overturn a deny**, and can only raise an `Ask` to an `Allow`. The
    fingerprint is checked **before** expiry and consumption, because re-approving cannot fix a digest
    that still will not match — reporting "expired" would tell a user to do exactly that.
  - **⚠ A refactor introduced a defect and a test caught it.** Extracting the evaluator into staged
    helpers (for `clippy::too_many_lines`) gave `resolve_approval` a `Result<(), PolicyReason>`
    signature, which cannot express "nothing matched" separately from "something matched and failed" —
    both are `Err`. The absent case returned `Err(DefaultAsk)`, which **overrode a tool's declared
    `Deny` and turned a refusal into a prompt**. The `ApprovalOutcome` enum now names the three cases,
    and `an_absent_approval_is_not_a_mismatched_approval` pins all three against one request shape.
  - **The decision maps to a tool error class**, and the mapping is where it is because getting it
    wrong produces a message a user cannot act on: an `Ask` is `tool.approval_required`, which prompts
    them, while a denial for a missing grant is `tool.permission_denied`, which does not. An expired
    approval maps to `tool.approval_expired` so a user re-approves rather than seeking permission they
    already hold.
  - 29 policy tests. 1263 workspace tests. All gates green. **DO NOT COMMIT.**
  **Not done, and deliberately not implied:** grants, approvals, and deny rules are **in-memory
  values passed in**, not persisted records with a lifecycle — durable approvals are `TLS-005` and the
  call ledger is `TLS-006`, so nothing writes them yet. There is no policy *store* (no per-workspace
  configuration, no versioning of rules), no budget or quiet-hours input despite the architecture
  listing them, and no user-facing explanation rendering — `PolicyReason` is the machine half of
  "explainable", and the human summary the contract asks for is a presentation concern with no
  consumer. `action_digest` is compared but **not computed**: the canonicalization and SHA-256 the
  approval contract requires (RFC 8785) is `TLS-005`'s, and inventing one here would put an
  unreviewed canonicalization under an approval fingerprint.
- [~] `TLS-005` Implement durable approval records and action fingerprinting. **The record, its lifecycle,
  the persistence, and the fingerprint *computation* are done. The `~` is now for two entries in the
  contract's fingerprint-input list that have no producer — "material content/artifact hashes" and
  "target connector account/resources" — so the mechanism covers every fact that exists yet and cannot
  cover two that nothing creates.**
  Evidence: `jarvis_domain::tool::approval`.
  - **The state machine is the contract's seven states with the transition table asserted cell by
    cell**, not through its happy path: `the_transition_table_permits_exactly_the_contracts_edges`
    walks all forty-nine pairs and requires seven legal edges and forty-two refusals, written out
    rather than derived, so adding an edge to the implementation fails here instead of silently
    widening what an approval lifecycle permits.
  - **⚠ The absorbing rule existed but was enforced nowhere, and only a mutation said so.**
    `can_transition_to` began `if self.is_terminal() { return false; }` and then listed the legal
    edges — and disabling that guard **compiled and passed every test**, because no arm in the table
    has a terminal source, so the branch was unreachable. It read exactly like enforcement while
    enforcing nothing, the same class as an unused constant. The rule is now a property of the table,
    and `has_any_transition` gives a test something to assert that a deleted guard would fail:
    `every_state_either_absorbs_or_can_be_left` requires a legal exit for every non-terminal state and
    none for every terminal one, in both directions.
  - **Lapse is computed at use, not trusted from the state column.** An approval that lapsed while
    nothing ran still says `Approved` in storage, so a check reading only the stored state would honour
    it. `covers` compares the expiry at the moment of the check, and the boundary matches the rest of
    the project: an expiry at `T` does not permit work at `T`. Falsified by removing the comparison.
  - `covers` gathers **all four** conditions that must hold — approved, matching digest, unexpired,
    and (via the state) unconsumed — so there is one definition of "this approval covers this action"
    rather than one per caller. Falsified twice: removing the state condition breaks a pending
    approval and a consumed one; removing the digest comparison would let a rewritten action through,
    which is the contract's "approving 'send this email' does not approve a rewritten recipient".
  - **An approval with no permitted channel is unrepresentable**, because one could never be decided
    and would sit `Pending` until it lapsed — the "no legal way out" shape this project has now found
    seven times. `AllowedChannels::new` refuses the empty set, and the channel a decision came from is
    checked against it, so recording `allowed_channels` is not a field with a reader and no effect.
  - **Time is not a decider.** `ApprovalActor` distinguishes a principal, an expiry, an invalidation, a
    consumption, and a withdrawal, so `decided_by` stays empty when time lapsed an approval — a record
    claiming a user expired one late at night would be an audit trail that lies. Non-decision
    transitions are deliberately not channel-checked, or expiry would be unreachable.
  - A preview item refuses **control characters**, not as cosmetics: a preview is rendered inside the
    prompt a user reads to decide, so it is the one place a crafted string could make the prompt
    misrepresent the action. A preview may be *empty* though, because whether a tool must show
    something is about the tool rather than a domain invariant.
  - **A first attempt at grouping the transition arms merged `Approved -> Rejected`, which is not a
    contract edge, and the exhaustive table test caught it immediately** — the reason that test writes
    the expected set out rather than trusting the implementation's own shape.
  - 25 approval tests. 1285 workspace tests. All gates green. **DO NOT COMMIT.**
  **Not done, and it is the half the TODO names second:** the **action fingerprint is not computed.**
    `action_digest` is a `String` compared for equality, and nothing produces one. The approval contract
    requires a *researched deterministic JSON canonicalization* (it names RFC 8785) plus SHA-256 with
    an explicit algorithm prefix, and both a new dependency decision and an official-specification
    review. `docs/research/integrations/` has no note for it and `evidence-manifest.json` has no entry,
    so implementing it now would be integration code with no evidence note — which `AGENTS.md` forbids
    outright. It needs a research round of its own, and inventing a canonicalization would put an
    unreviewed one underneath every approval in the product.
    **The fingerprint is now computed, and the research round it needed is
    [rfc8785-canonicalization.md](docs/research/integrations/rfc8785-canonicalization.md)**
    (`IMPLEMENTATION_READY`, manifest entry `jcs-canonicalization`). No crate was adopted: RFC 8785 is
    implemented directly, and the SHA-256 that turns the canonical form into a digest uses the
    `sha2` `0.10.9` already resolved and already reviewed for this boundary. The split is
    `jarvis_domain::tool::canonical` (the canonical form, a pure function of values) plus
    `jarvis_infrastructure::tool_fingerprint` (the hash), because hashing is a concrete implementation
    the domain layer must not depend on — the same division `SchemaFingerprint` records.
    - **The one thing it refuses is numbers, and the refusal is the security decision.**
      RFC 8785 §3.2.2.3 defers number serialization to ECMA-262 and says the algorithm "is **not**
      included in this document"; a conformant serializer therefore needs shortest-round-trip double
      formatting, and a subtly wrong one produces a digest that is **stable inside one build and
      different from every other implementation** — every internal test passes while a client previewing
      the same action is refused. Since the fingerprinted document is JARVIS's **own** envelope, every
      value contributed is a string, so `FingerprintInput` makes a number (and a boolean, `null`, an
      array, a nested object) **unrepresentable**. The `NaN`/`Infinity` abort the RFC requires is then
      unreachable rather than unimplemented, and no general-purpose JCS function is exported — an
      honest narrowing recorded in the contract's Implementation Status.
    - **Sorting is by UTF-16 code unit, not by byte, and the test proves the difference.** §3.2.3
      specifies code units and notes a UTF-8 sort "would differ and thus be incompatible". For ASCII the
      two agree, so a byte-wise implementation looks correct until the first non-ASCII key — and a
      `BTreeMap` iterates in Rust's byte-wise `Ord`, which is exactly the wrong order. The names are
      therefore sorted explicitly. **Falsified**: substituting `left.cmp(right)` fails the RFC's own
      §3.2.3 vector *and* the astral-plane test, printing the wrong order
      (`…"דּ":…,"😀"` where the RFC puts the emoji before the Hebrew letter).
    - **The escape table is asserted rather than described.** §3.2.2.2 requires the five characters with
      short forms to *use* them and every other control character to be lowercase `\uhhhh`; the two are
      different byte strings, so "it unescapes to the same character" is not sufficient. Only `"` and
      `\` are escaped and **every other code point is emitted as-is**, which is the rule that is easy to
      over-apply. **Falsified**: emitting `\u0009` for a tab fails
      `a_control_character_uses_the_short_form_and_lowercase_hex`.
    - **The digest is a typed value at every boundary, which closed a second hole.** `action_digest` was
      a `String` in both the durable record and the policy request, so a fingerprint was *compared* as
      text and nothing validated its shape. `ActionDigest` now parses the contract's `sha256:<hex>` form
      (64 **lowercase** hex, algorithm included so a digest from another function cannot compare equal),
      its `Deserialize` goes **through** `parse` so a value arriving over the wire is held to the same
      rule as a stored one — the systemic defect `wire_validation_tests` records for nine other types —
      and the handler parses it at the trust boundary. A malformed fingerprint is `request.invalid`
      rather than `approval.fingerprint_mismatch`. **Falsified**: falling through to the comparison
      instead of refusing reports the misleading `approval.fingerprint_mismatch` for `md5:…`, which
      sends a user to re-approve an action whose fingerprint they never could have computed.
    - **⚠ The first envelope omitted `effects` and `risk`, and reading the contract's own input list
      against the implementation found it.** `ToolIdentity::schema_fingerprint` covers the **input schema
      only**, so a tool reclassified from `read_only` to `destructive` keeps its capability, its source,
      and its schema — and therefore kept its *fingerprint*, which meant an approval granted for the read
      authorized the delete. The contract requires the fingerprinted object to contain "effects and
      constraints" and the tool fabric names "effect/risk classification" in what an approval binds to,
      so both are envelope fields. The effects list is additionally canonicalized **as a set** (sorted,
      deduplicated through the contract spellings), because a fingerprint that varied with list order
      would refuse an approval for the action the user actually reviewed. **Falsified twice**: omitting
      the two fields fails 3 domain tests and 1 digest test; concatenating the spellings instead of
      joining them fails the set-uniqueness test.
    - `jarvis-domain` 20 canonical tests (RFC §3.2.3 vector asserted **by value**, whitespace absence,
      the escape table, determinism, input-order independence, a one-character change, the reclassification
      case with the identity held fixed, the effect-set canonicalization, the bounds, and the
      wire-validation rule) and `jarvis-infrastructure` 7 (a NIST SHA-256 **known-answer** vector, that the
      digest is over the canonical form rather than `serde_json`'s output, and the reclassification case at
      the digest level).
    - **Still not done:** cross-language fingerprint vectors (the Rust half is covered; a second
      language's implementation is the increment). Two entries in the contract's input list remain
      uncovered because nothing produces them: "material content/artifact hashes" and "target connector
      account/resources" — each is a new digest input when the executor arrives, not a new rule.
    **Persistence is now done**, which was the other named gap. `000008_approvals.sql` adds `approvals`
    and `approval_transitions`, `jarvis_application::repository::approval::ApprovalRepository` is the
    port, and the schema document records the as-built shape and the four ways it differs from the
    original design — `tool_identity_json` over split columns, `decided_by` plus `decided_via` over one
    principal, the summary and preview stored **verbatim** because they are the record of what the user
    was shown, and `state` with **no default** because a defaulted `pending` is the fail-open direction.
    `an_upgrade_adds_the_approval_tables_without_touching_what_is_there` asserts both halves of an
    additive migration: the tables exist **and** a pre-existing row survives, because a migration that
    added tables while breaking an existing one would pass the first half alone. It also asserts the
    `NOT NULL`-without-default property by attempting a state-less insert, since that is a fact about
    what the writer can do rather than about what the catalog says.
    Also not done: there is no
    approval list/decide API (`TLS-013`) or channel-assurance check (`ACC-027`). The `preview` is
    structured but **not redacted**: the contract requires it redacted, and only a tool's producer knows
    which of its values is sensitive, so redaction belongs where a tool builds its own preview.
    **The SQLite adapter is now built**, which was the last named gap in this item:
    `jarvis_infrastructure::storage::approval_repository::SqliteApprovalRepository` implements the port
    over `Database::open_in_memory()` in the 21 tests below, and a decision writes its state change and
    its `approval_transitions` row in **one transaction**, so the trail cannot describe a decision that
    is not durable or omit one that is.
    - **The reader walks the state machine rather than assigning the stored state.** `stored_approval`
      rebuilds through `DurableApproval::request` and then drives `PENDING -> ... -> stored_state`, so a
      row claiming `PENDING -> CONSUMED` is `storage.row_corrupted` rather than a reconstruction. That
      distinction is the whole point: assigning the fields would accept the row and hand back a *spent*
      approval that was never approved, which is the worst possible reconstruction of a decision. The
      walk derives the version, and the derived value is cross-checked against the stored `version`
      column, because a row whose state and version disagree records a transition count its state does
      not explain. **Removing that cross-check compiled and passed every test** — no other case used an
      inconsistent pair — so `a_stored_version_the_walk_cannot_derive_is_corruption` exists to make the
      comparison reachable, which is the same "reads as enforcement while enforcing nothing" shape
      `TLS-005`'s absorbing guard had.
    - **⚠ The adapter read the state and the identifier it *generated*, and two mutations found it.**
      `stored_approval` built the value through `DurableApproval::request`, which mints a v7
      identifier, and returned it without overwriting the generated one — so every `load` handed back a
      record whose identity was newly minted, and a caller that loaded an approval and then decided it
      named a row that does not exist. The round-trip test caught it because it compares the
      **identifier** as well as the fields, and `assert_eq!(loaded, requested)` would have failed for
      that reason while reading as a field mismatch. The second: `AlreadyInState` was conditioned on the
      caller's `expected` version, and both taps of a double-tap carry the same `expected`, so the
      branch was **dead** — the repeat was refused with a version conflict, which is exactly the lie the
      variant exists to prevent. The condition is `stored_version == transition.version`, because that
      version is the one the transition produces.
    - **A decision's instant comes from the row, per step.** The first version passed the approval's own
      `expires_at` as every step's `occurred_at`, so a stored approval decided at 12:00 was
      reconstructed as decided at its 12:10 deadline — wrong by ten minutes, and invisible to any
      assertion on the state or the version. `StoredDecision::instant_for` reads `decided_at` for a
      decision and the row's `updated_at` otherwise, and refuses a decision row with no recorded instant
      rather than defaulting, because `apply` always writes one.
    - **`DecideOutcome::AlreadyInState` carries no version, and that is a correction.** It held the
      version the caller stated, which the adapter bound from `expected` — always equal to the input and
      therefore never wrong in a way a test could see. The two ways to make it informative are both
      false statements, so the field is gone rather than filled with a guess.
    - **Every reconstruction arm is covered, not just the two the round trips reach.** Six states are
      asserted through one table test, because an arm naming the wrong actor or the wrong path length
      would produce a different state or version — `invalidated` is the interesting one, two steps from
      `PENDING`, so a reader that put every terminal state one step away would refuse the row or land on
      version 2. Seven mutations were run against the reader and the writer; six were killed, and the
      one that survived is the version cross-check named above, which is why that test exists.
    - 21 approval-adapter tests. **DO NOT COMMIT.**
- [x] `TLS-006` Implement idempotent tool-call ledger and execution state machine. Owns
  `jarvis_domain::tool::ledger`.
  Evidence: the eleven contract states with their transition table, a reservation keyed by **all five
  dimensions**, and the `ACC-025` classification as one executable sequence.
  - **`ACC-025` is a test, not a sentence.** *"Crash after the fake provider accepts an effect but
    before result persistence. Recovery enters reconciliation and proves no duplicate effect."*
    `a_crash_after_dispatch_enters_reconciliation_and_never_a_retry` walks it: a call reaches
    `EXECUTING` (the state the contract requires **before** an external effect), the process stops, the
    startup scan finds it, the classification is `Reconcile` and **not** a retry — and a duplicate
    submission meanwhile is reported as *unsettled* rather than granted. Only after the provider is
    read and confirms nothing landed does a dispatch become permissible, so the permission comes from
    **checking the world**, not from elapsed time or from the tool declaring itself idempotent.
  - **⚠ A real defect the tests found, of exactly the class this round is about.**
    `may_have_effected()` was derived as `state.was_dispatched()` — a question about *where the call is
    now*. A call that reached `EXECUTING` and then ended `FAILED` is no longer in a dispatched state,
    so the answer became `false`, and a dispatched call that failed would have been treated as having
    provably had **no effect** and retried — duplicating the effect. That inverts the one conclusion
    `ACC-025` depends on. Dispatch is now recorded as a **fact** (`dispatched_at`, stamped on the first
    entry into `EXECUTING`), because a thing that happened once is not recoverable from a state that
    has since moved on — the same lesson as `TLS-004`'s approval lapse.
  - **Reconciliation is a transition, not a flag.** The first version set the no-effect claim and left
    the row in `RECONCILING`, so the row still looked unsettled to every reader — including the
    reservation check, which reported a duplicate as needing reconciliation *after the reconciliation
    had happened*. `confirm_no_effect` now ends the call as `Failed` (it did not succeed) with the claim
    recorded, and the claim is **sticky** (OR-assigned) so no later transition can un-confirm an effect
    the provider was already read about.
  - **A pre-dispatch refusal and a post-dispatch failure are distinguished by one fact**, asserted as a
    pair over two calls whose only difference is where they were refused: the first proves no effect
    (nothing reached the provider) and the second proves nothing (the effect may have landed). A
    single-assertion test could not show the difference, and the failure of that pair is a duplicate
    effect.
  - **⚠ `ToolCallState` had two spellings for one value.** `as_contract_str` returned the contract's
    uppercase prose (`EXECUTING`) while the serde derive produced `snake_case` (`executing`), so a state
    had one spelling to parse and another to store — the "two spellings denote one value" defect this
    project refuses for identifiers, in the enum that records whether a side effect happened. It
    surfaced because a test asserted the serialized bytes. One spelling is now chosen (the lowercase
    one, which is what the contract's own JSON examples show) and
    `every_state_serializes_to_its_own_spelling` asserts the method and the derive agree for **every**
    variant, so neither can drift.
  - **`ReservationOutcome` has four variants rather than a `Result`**, because "an equivalent call
    already exists" is not a failure — it is the point of a reservation — and the three duplicate
    shapes need different responses: read the answer, wait, or **reconcile and never retry**. A
    `Result<(), Error>` collapses them into "no", and a caller given "no" retries, which is the single
    most dangerous response available here. `permits_dispatch` is the one definition of the retry
    question.
  - The transition table is asserted **cell by cell** (twenty-two legal edges against a hundred and
    twenty-one pairs) with the expected set written out by hand, and `Executing -> Executing` is
    refused so a second dispatch cannot be recorded while the reservation still looks held.
    `Approved -> Executing` is refused too, so the reservation cannot be skipped.
  - **The attempt number is private** so a zero attempt is unrepresentable — "attempt zero" has no
    meaning, and the first version left the field public and normalised `0` in `reserve`, so a caller
    could put one in afterwards.
  - 30 ledger tests. 1318 workspace tests. All gates green. **DO NOT COMMIT.**
  **The persistence gap is now closed.** `000009_tool_call_ledger.sql` adds `tool_call_records` and
  `tool_call_transitions`, and
  `jarvis_infrastructure::storage::tool_call_repository::SqliteToolCallRepository` implements
  `jarvis_application::repository::tool_call::ToolCallRepository` over a real migrated database — 22
  contract tests. The named sentence this slice exists to make true is the module's own:
  *"the reservation is atomic only within one process's memory: the adapter that makes it atomic across
  processes is part of the persistence work"*.
  - **The reservation is now one statement whose outcome is the verdict.**
    `reserve` performs a single `INSERT` against the unique index
    `(workspace_id, principal_id, tool_identity_json, idempotency_key)` and classifies the result: a
    unique violation means **the row that won is read** and classified *after* the constraint fired,
    when it is durable. There is no read-then-decide-then-insert window, which is what an in-memory map
    cannot close. `two_connections_racing_the_same_key_produce_one_grant` reserves from **two separate
    connections on one file database** — two independent SQLite sessions, the closest reproduction of
    two processes on one host. A version using two repositories over one pool would pass for a
    read-then-write adapter, so it would not test the property at all.
  - **The unique index is over serialized identity *text*, so its soundness rests on a serializer.**
    `ToolIdentity` is a struct of fields serde writes in declaration order, which makes equal identities
    byte-identical — and a representation that could reorder (a map, or a set field) would make the index
    silently stop deduplicating while still existing. `equal_identities_serialize_identically_...`
    asserts the determinism **and** that two independently constructed equal identities collide in the
    store, because an assumption a uniqueness guarantee rests on is not one to believe.
  - **⚠ A stored row is *validated*, not replayed, and the reason is the state machine.** The approval
    reader walks the transition table because an approval's version is derivable from its final state.
    Here it is **not**: `REQUESTED -> VALIDATED -> RESERVED -> EXECUTING -> SUCCEEDED` and the same path
    through `WAITING_APPROVAL` both end `SUCCEEDED` at versions 6 and 7, so the version records how many
    transitions *actually* happened and no final state implies it. The guarantees therefore come from
    validation in `LedgerEntry::restore`, which the adapter maps to the column it concerns:
    a non-zero attempt, a version at least the first, a terminal row with an outcome, and **a state that
    requires a dispatch recording one**.
  - **The dispatch invariant is the one with teeth.** A stored `SUCCEEDED` with a `NULL` dispatch column
    claims an effect happened without the dispatch that caused it, and reading it as harmless is exactly
    the inversion `TLS-006` fixed: `may_have_effected()` answering `false` for a dispatched call is what
    retries a call whose effect may have landed. `a_dispatched_row_with_no_dispatch_instant_is_corruption_not_harmless`
    asserts the refusal.
  - **Seven mutations, all killed, and the first attempt at the test helper was itself wrong.** The
    helpers originally walked `REQUESTED -> EXECUTING` directly, which is **not an edge** — the test failed
    for a reason that was about the helper rather than the adapter. They now derive a path by
    breadth-first search over the domain's own `can_transition_to`, so a helper cannot disagree with the
    machine it exercises. That is the same rule as a test restating a constant.
  - The migration test asserts the half specific to this slice: **the unique index actually refuses a
    duplicate**. The fixture's first version generated a fresh `principal_id` per row, so the two rows
    differed in a *key* column and the insert rightly succeeded — the assertion caught the fixture's own
    mistake, which is why it is written as an insert rather than as a catalog query.
  - 22 ledger-adapter tests, 10 migration tests. **DO NOT COMMIT.**
  **The recovery pass now exists, and it is wired into startup.** `jarvis_application::tool_recovery`
  calls the classification and the work list that had **no caller**, and `jarvisd` runs it before
  readiness beside the run pass. What it fixes is concrete: a call that reached the provider and then lost
  its daemon stayed `EXECUTING` for ever, and the next attempt's reservation found it and answered
  `InFlight` — **telling a caller to wait on a process that is gone**. The pass converts that dead
  in-flight call into a `RECONCILING` work item, which is the state that says "the outcome is unknown and
  must be established before this is repeated", so a duplicate is reported as `Unsettled` rather than
  `InFlight`. A call that never dispatched ends `CANCELLED`: nothing can have effected, and leaving it
  non-terminal would make a later reservation wait on it.
  - **⚠ The classification's own target was an illegal edge, and nothing noticed because nothing called
    it.** `classify_interrupted` answers `Reconcile` for a row found in `RECONCILING` as well as one found
    in `EXECUTING` — correctly, since both may have effected — but `RECONCILING -> RECONCILING` is not an
    edge: the table refuses a no-op so a second consume cannot record progress while the effect stays
    single. Its own test drove the `EXECUTING` case, where the edge is legal, so the case was unasserted.
    `InterruptedCallAction::needs_write` now states the one question a writer has to ask, and the domain
    suite **still passes with the fix reverted** — which is the measurement that shows the defect was
    latent rather than live. Reverting it fails five application tests.
  - **⚠ A mutation found a hung startup path, and it was two defects behind one another.** Making
    `needs_write` always `false` did not fail — it **hung**. The cause was that the pass paged over a set
    containing **its own output**: converting an `EXECUTING` row produces a `RECONCILING` row that the
    work list still returns, so "did this page change anything" never became false, and a store with more
    than one page of stranded calls reported `incomplete_store` for ever — which the daemon turns into a
    **refusal to start on every restart**. Two corrections followed: the drain signal is rows *converted*
    rather than rows *examined* (a full page of already-reconciling rows is work finished, not a stall,
    while a full page whose every write was refused is the genuine stall), and the pass pages
    `awaiting_conversion`, which excludes `reconciling` so the set strictly shrinks. **The predicate's
    original form selected `EXECUTING` alone and reached only two of the six states the classification
    answers for — see the review correction below.** The second defect
    was a *silent stranding*: with the old predicate, a second page could hold only already-converted rows
    and the rest were unreachable — measured as **500 of 505 converted**, not a hang.
  - **Two scans, each with one meaning.** `possibly_effecting` is the outstanding-work list a caller
    reports and a provider read walks; `awaiting_conversion` is the conversion input. One method whose
    meaning depended on the caller is what produced the defect above.
  - **`bounded` is the probe row, not an inference.** The adapter reads one row more than the page size,
    so "there is more" is distinguishable from "that was all" — the same technique the run recovery page
    records, and for the same reason: this bound hides the **newest** rows.
  - **12 recovery tests, 23 ledger-adapter tests** (one of which drives the pass against a **migrated
    database** rather than a double), **+1 migration assertion**. Four mutations, all killed — including
    the probe row and the illegal-edge fix. **DO NOT COMMIT.**
  **⚠ Review correction: the conversion scan reached two of the six states the classification answers
  for, so `report.cancelled` was dead and a stranded pre-dispatch reservation was never settled.** The
  pass pages `awaiting_conversion`, and the predicate read `state = 'executing'` — yet
  `classify_interrupted` answers `SafeToRetry` for five pre-dispatch states (`requested`, `validated`,
  `waiting_approval`, `approved`, `reserved`). So a call that **claimed its reservation and then lost its
  daemon** was never offered to the pass: its dead holder kept the unique-index entry, and every later
  retry of that key was answered `InFlight` — *waiting on a process that is gone*, which is the exact harm
  the pass exists to remove, applied to the states it could not see. Two things hid it: the `cancelled`
  counter was unproducible (so nothing reported the gap), and
  `a_call_that_never_dispatched_is_cancelled_rather_than_reconciled` was **named for the behaviour and
  asserted its absence** — its body checked `cancelled == 0`, and its comment explained that the
  *reporting* scan omits undispatched rows, which is true of `possibly_effecting` and false of the
  *conversion* scan the pass pages. The test encoded the bug.
  - Fix: the predicate is every non-terminal state **except `reconciling`** — the exclusion is what keeps
    the paging terminating, and it is exactly one state, so the classification has somewhere to be
    applied. Every selected state still strictly shrinks (pre-dispatch → `cancelled`, terminal;
    `executing` → `reconciling`, excluded).
  - **A terminal target now carries its outcome.** `Cancelled` is written on the cancelled transition,
    because `apply` records `None` when handed `None` and a terminal row with a `NULL` outcome is refused
    as corruption by `LedgerEntry::restore` and the adapter's reader — so a pass that wrote one would
    produce a row the next start cannot read.
  - **The unproducible `awaiting_reconciliation` counter is deleted.** It was incremented only when
    `was_in == Reconciling`, which the predicate excludes, so it could never be non-zero; `examined()` now
    equals `changed()`. A producer-less field is the shape this project removes, not one to keep in case.
  - **The predicate is asserted against the domain's own state set**:
    `the_conversion_scan_covers_every_state_the_pass_can_settle` places a row in every non-terminal state
    and requires the scan to return exactly the states that need a write — six — because the SQL string
    and the Rust table compile independently and drifted once already.
  - **`drive_to` now derives its path by breadth-first search** over `can_transition_to` rather than
    listing edges, after a hand-written path produced `REQUESTED -> APPROVED`, which is not an edge; the
    fixture failed for a reason about the helper, the same way round 93's did.
  - Falsified both halves: restoring the narrow predicate fails the new adapter tests
    (`left: 0, right: 1`, and `["executing"]` against six states); dropping the outcome fails
    `left: None, right: Some(Cancelled)`. **DO NOT COMMIT.**
  **Still not done, and named:** nothing *else* calls the ledger — there is no executor, and the
  controller still refuses a tool intent with `run.tools_not_implemented`. **The pass does not reconcile
  and does not retry**: reconciliation means reading the provider to establish whether an effect landed,
  which needs a tool invocation this build does not have. So it converts a dead in-flight call into a work
  item and leaves the provider read to the slice that can make one. There is no attempt-count ceiling (the
  retry budget is `BRN-008`'s), no per-call deadline in the row, and no outbox event per transition — that
  is `AUT-004` and `ACC-044`, whose test asserts state and outbox never diverge. The design's
  `arguments_ref`, `result_ref`, `step_id`, and `runtime_call_ref` columns are not in the as-built table
  because each needs an artifact store, a step writer, or an external runtime, none of which exists — so
  the row records the *reservation and its outcome* and not the call's payload.
- [~] `TLS-007` Implement safe reference filesystem read and write-plan tools. **The authorization half
  is done; the enforcement half is not, and cannot be without a dependency decision — the `~` is the
  honest state.**
  Evidence: `jarvis_domain::tool::path_grant` implements the security architecture's filesystem rule —
  *"Canonicalize paths and defend against traversal, symlink/junction/reparse-point races, alternate
  data streams, reserved names, and case differences"* and *"File grants are rooted and mode-specific:
  read, create, modify, delete"*. It is a **pure function of values**, so the rules are exhaustively
  testable without a filesystem and the adapter that performs a read has no policy of its own to get
  wrong.
  - **The mistake the module is written around: containment compared as a string prefix.**
    `/data/notsecret` starts with `/data/note` and is a sibling directory, so a prefix test authorizes a
    path the grant never covered — at the authorization layer that is a **read of a file the user never
    granted**. Containment is compared segment by segment, which is only sound because a value cannot
    exist in un-normalized form. The falsification removes the segment comparison and **six tests fail**,
    including `a_sibling_sharing_a_string_prefix_is_refused_by_authorization`.
  - **`..` is refused rather than resolved.** Resolving is possible and would give the right answer for a
    purely lexical path, but it discards the fact that the caller asked to leave the directory — and once
    symlinks exist the lexical answer and the filesystem's answer differ. Refusing keeps the decision
    independent of the filesystem, which is what makes it testable at all. The specific reason
    (`Traversal`) is reported, because `OutsideEveryRoot` would invite the user to grant access, which is
    not the missing thing.
  - **The case comparison is an input, because the filesystem decides it.** Whether `/Data` and `/data`
    are one directory is a fact this layer cannot see. Assuming case-insensitive refuses legitimate paths
    on Linux; assuming case-sensitive lets a Windows path evade a grant by changing one letter — the
    "case differences" case the architecture names. `PathComparison` is asserted **both ways on one
    pair**, and a case-only difference is reported as `CaseMismatch` rather than `OutsideEveryRoot`
    because the user's fix is to correct a letter.
  - **The longest matching root wins**, not the first: "read the whole tree, write one directory inside
    it" is a legitimate configuration, and first-match would let the broad root's modes apply inside the
    narrow one. Order-independence is asserted, the same property `TLS-004` requires of policy.
  - Per-platform evasions each asserted one at a time, since each defends a different behaviour:
    reserved device names **whatever the extension or case** (`CON`, `con.txt`, `AUX.tar.gz` — Windows
    resolves them anywhere, so a whole-segment or exact-case check lets them through on the one platform
    where they matter; `console.log` and `conartist.md` stay usable, so the rule is the stem not a
    substring); the backslash, which is a separator on Windows and a filename character on Unix; a colon,
    which introduces an alternate data stream; and a trailing dot or space, which Windows silently strips
    — each of which makes **two strings name one file**, the defect class this project refuses for
    identifiers.
  - **⚠ Two defects, one found by a test and one by an assertion about the wire form.**
    `differs_only_by_case_from` returned `true` for a path **inside** the root, so an ordinary descendant
    was reported as a case mismatch — it needed both halves (the prefix must match ignoring case *and*
    not match exactly), and the missing half is exactly the case that made the predicate useless.
    **And `#[serde(transparent)]` bypassed validation**: the derived deserializer wrapped the string
    directly, so `"data//notes"` or `"../escape"` arriving over the wire produced a value that violates
    the type's own invariant — which everything downstream trusts, since `authorize_path` skips the shape
    check for a value it believes is normalized. `Deserialize` is now hand-written to go through
    `parse`, and a falsification that skips validation fails the round-trip test.
  - 31 path tests. 1349 workspace tests. All gates green. **DO NOT COMMIT.**
  **Not done, and it is the half that makes this safe in production: nothing opens a file.** The rules
  are enforced on values, and the actual read must use open-relative/no-follow primitives because a
  check-then-open has a TOCTOU race the architecture explicitly names — *"Use open-relative/no-follow
  primitives or helper process boundaries where platform support requires them"*. This workspace denies
  `unsafe-code` (`[workspace.lints.rust]`), so the `openat`-family syscalls are unreachable from it, and
  `cap-std` is not a dependency. Implementing the read therefore needs a dependency decision plus its
  research gate: an official-source and license review, and either a routine-dependency-ledger row or a
  full evidence note. Shipping a canonicalize-then-open check while calling it safe would be worse than
  shipping the decision alone, so the gap is named here. Also absent: symlink and reparse-point
  detection (which needs the metadata the adapter would read), a write *plan* with approval preview, the
  tools themselves, and any grant store — `PathGrant` values are passed in, not persisted.
- [x] `BRN-056` Sweep the migrations for columns nothing references, and name the dead policy-layer
  machinery a doc claimed was in use. Two documentation defects found by two sweeps, no behaviour
  changed.
  Evidence: **a corrected sweep of the migrations against the Rust sources** (207 surviving columns
  across 17 tables, after processing migrations in order so a table a later one drops and recreates is
  not counted with its old shape) still finds **nine** columns mentioned nowhere in `crates/` or
  `apps/`. Seven are already documented as absent — the `agent_steps` group belongs to the table
  `docs/data/schema.md` marks "**Still schema-only**", and `application_locks` is explicitly "created
  but unused". **Three were not:** `agent_runs.result_ref`, `agent_runs.error_ref`, and
  `agent_runs.plan_summary_ref` were listed in the `agent_runs` field list as ordinary members of the
  record, while the same document carefully flags `context_manifest_id` and the `runtime_id` pair as
  unpopulated. A reader of that list would take them for implemented. They are now called out, with
  the reason each is absent (two need an artifact store; the third needs a plan, and nothing writes
  one) and with the note that `error_code` *is* populated beside `error_ref`, which is what makes the
  distinction visible.
  - **⚠ The first version of the sweep was wrong, and the false positives were the useful signal.**
    It concatenated every migration and reported eleven orphans, including `granted_by` and
    `scope_ref` from `model_policy_exceptions` — a table `000005` **drops and recreates** with a
    different shape. A sweep that over-reports is worse than none, because it is read as a work list.
    The rewrite tracks `DROP TABLE` and replacement `CREATE TABLE` in version order and re-run it
    gave nine.
  - **The second defect: `ResolvedPolicy` is dead machinery that a contract table described as
    live.** `PolicyLayer`, `PolicyLayerContribution`, `ResolvedPolicy::merge`, and
    `narrowing_layers` implement the contract's six-layer precedence — and a grep of `crates/` and
    `apps/` finds **every call site in `jarvis-domain`'s own test module**. The real path is
    `PolicyService::put`, which merges through `PolicyRules::merge_stricter` directly, so the rules
    are enforced while *which layer narrowed them* is dropped at creation. `model-data-policy.md`'s
    test table listed `ResolvedPolicy::merge` as the enforcer of "the precedence order is the
    contract's order", and `narrowing_layers`' own doc claimed it was "used by operator output" —
    while the CLI prints only the *configured* policy id from its config file and
    `ActivePolicyResponse` has no layer field at all. Both claims are now recorded as false rather
    than left to mislead, and the type is kept because it is correct and its test falsifies a wrong
    version.
  - **Closing the layer gap is a named slice, not a wire-up**, and that is why this round recorded
    it rather than half-implementing it: it needs a contribution recorded at version creation, a
    **migration** (`model_data_policies` stores `rules_json` and has no provenance column), a read
    path, and a wire field — four parts, each a real change, against a contract requirement
    ("`GET` returns ... source layers") that is presently unmet.
  - 1507 tests (unchanged — no behaviour changed), fmt/clippy/doc clean, both docs gates green. The
    two doc corrections are in `docs/data/schema.md`, `docs/contracts/model-data-policy.md`, and the
    two `jarvis-domain` doc comments whose claims were false.
  - **Still not done:** the layer provenance above; `agent_runs.result_ref`/`error_ref` and
    `plan_summary_ref` (each needs a producer that does not exist).
- [ ] `TLS-008` Refresh MCP evidence; implement stdio and Streamable HTTP client.
- [ ] `TLS-009` Implement scoped authenticated MCP server export.
- [x] `BRN-057` Close the layer-provenance gap `BRN-056` named: record, persist, read back, and serve
  the source layers a policy version was merged from, so the contract's `GET` requirement stops being
  unmet. Behaviour change: one nullable column, one new wire field, and a producer that was absent.
  Evidence: **the `narrowed` flag is falsified by removing the producer.** Making `PolicyService::put`
  record an empty provenance instead of its two contributions fails
  `a_policy_write_records_and_reports_the_layers_that_narrowed_it` with `source_layers: []`, which is
  what proves the four parts are connected rather than four changes that each compile. The test drives
  `PUT` twice against a running router and asserts `["workspace"]` for a first write, then
  `["workspace", "task-restriction"]` in precedence order for a second, and then re-reads through
  `GET` to require the two responses to describe the same version. The adapter half is asserted
  separately by `a_versions_layer_provenance_round_trips_and_an_absent_one_reads_as_empty`, which
  stores NULL explicitly — the not-back-filled case a fresh schema never exercises.
  - **Why this was four changes and not a wire-up:** `model_data_policies` stored `rules_json` alone,
    so provenance had nowhere to live. `000011_policy_version_layers.sql` adds a nullable
    `layers_json`, deliberately **not back-filled**: a row written before provenance was kept genuinely
    has none, and defaulting it to `["workspace"]` would attribute a narrowing to a layer nobody
    observed. That is why the wire field is `#[serde(default)]` rather than required — an absent
    provenance and an unreported one are the same statement to a client.
  - **The `narrowed` flag is computed, not assumed.** `source_layers_view` re-folds the recorded
    contributions through `ResolvedPolicy::merge` and `narrowing_layers` rather than marking every
    contributor, because a layer that contributed a **permissive** ruleset is in the list without
    having imposed anything; naming it as the cause would send an operator to change a policy that is
    not responsible. The list is sorted by `PolicyLayer::ORDERED` position rather than by insertion,
    so the strongest layer reads first; an unrecognised name sorts last instead of being dropped,
    because dropping it would hide a layer from the operator who asked which one is responsible.
  - **Two limits recorded rather than repaired.** `narrowed` is a claim about the fold and not about
    intent (a layer repeating an earlier restriction reads `false`), and a **contradictory persisted
    provenance degrades silently**: `source_layers_view` maps a failed merge to an empty narrowing set
    and `narrowing_layers` skips an erroring step, so every layer would read `narrowed: false` — not
    the same statement as "no layer narrowed". `PolicyService::put` refuses a contradiction before
    recording it, so no version it wrote can reach this state; the residual risk is a writer outside
    it. The wire field is a bool and cannot carry "unknown", so this is named in
    `docs/contracts/model-data-policy.md` instead of being papered over.
  - **Doc claims corrected in the same change, because this slice made them false the other way:**
    `ResolvedPolicy`'s "no production caller" header and `narrowing_layers`' "used by operator output"
    are both replaced with what is now true; the contract's "recorded rather than repaired" paragraph
    is replaced by the four parts; the contract test table row for precedence names the live caller;
    `docs/data/schema.md` documents the column and the nil-not-backfilled decision.
  - 1509 tests (+2), fmt/clippy/doc clean, both docs gates green.
  - **Also asserted at the composed daemon surface**, which is where this defect class is invisible to
    a router test: `tests/e2e/policy-surface.mjs` §7b drives two writes against a real `jarvisd` and
    requires the second to report `["workspace", "task-restriction"]` in precedence order, that only
    the workspace entry reads `narrowed: true`, and that the workspace entry names stored version 1
    while the submission omits `policy_version`. **Falsified by hard-coding `narrowed: true`**, which
    fails the journey with "only the workspace layer narrowed" — so the flag is not decoration.
  - **The first draft of the journey check was wrong and failed against a correct implementation.** It
    required both contributors to be marked, and the second write submits a *looser* locality than the
    one in force, so `task-restriction` genuinely narrows nothing. That is the field working as
    designed, and the episode is recorded in the contract doc because a test that wanted every
    contributor marked would have passed against the shortcut and made `narrowed` meaningless.
  - **Not closed by this slice:** a *route decision* still cannot carry a layer list — no field on
    `RouteRequest` or `ModelRouteDecision` can hold one — so a model call can report the rules that
    applied but not which layer narrowed them. That half of `ResolvedPolicy`'s own doc remains
    aspirational and is left stated rather than implied.
- [x] `BRN-058` Make a run's consumption durable and readable: two `model_calls` columns were bound on
  every write and named by no `SELECT`, so the run record's "model/tool usage and budget state" had no
  read path and the ceilings were compared against a single live call.
  Evidence: **falsified by removing the two columns from `model_call_columns!`**, which fails four
  tests — `reported_usage_and_its_lifted_cost_round_trip_through_real_columns`,
  `a_runs_calls_load_across_logical_calls_and_in_the_order_they_happened`, and both new HTTP tests
  (`a_run_read_reports_the_usage_its_calls_recorded`,
  `a_run_with_no_recorded_usage_omits_the_field_rather_than_zeroing_it`). **Falsified again** by
  replacing the run predicate with `(run_id = ? OR 1 = 1)`, which fails the run-scope test naming the
  leaked foreign call (`input_tokens: Some(1000)`).
  - **The defect was two-layered, not one.** `usage_json` and `estimated_cost_microunits` existed from
    the first migration and `record_outcome` bound both; the port's own `StoredModelCall` had no field
    for either and `model_call_columns!` omitted them. So the value was written, the type could not
    carry it, and no reader could ask for it — three gaps that each look like "not wired up".
  - **An existing test worked around the hole and documented the workaround as a design decision.** The
    round-trip test read `usage_json` with **raw SQL** and justified the detour with "`StoredModelCall`
    deliberately exposes only what a caller needs and usage is not part of it". The test directly below
    it is about the *same* defect class for `finish_reason` — a column bound by the write and named by
    no `SELECT` — and calls it a defect. That comment is now replaced with an assertion through the
    port, which is what kept this from being noticed.
  - **`Usage::summed` puts the arithmetic in the domain.** An unreported counter stays unreported
    through the fold (`None + None` is not a measurement), a `Some` beside a `None` keeps the
    measurement, and `provider_reported` is the **conjunction** so one estimated contribution makes the
    total estimated. `summed` returns `None` for an empty sequence rather than a zeroed `Usage`.
  - **The run scope is asserted against a *second run*.** The test creates another run — `model_calls.run_id`
    is a foreign key, and `record_attempt` maps an insert failure to a `logical_call_attempt` conflict,
    so referencing a run that does not exist was refused as a uniqueness violation rather than the
    missing parent it is.
  - **A mutation that was not a mutation.** My first bypass attempt replaced the predicate with
    `run_id = ? OR ? IS NOT NULL` and the test **passed** — because `sqlx` leaves the now-unbound third
    placeholder as NULL, and `NULL IS NOT NULL` is false. A falsification that does not fail is a signal
    to check the mutation was real; the literal `1 = 1` failed correctly.
  - 1514 tests (+5), docs synced in the same change.
  - **Also asserted at the composed daemon surface, in the direction that is reachable there.**
    `tests/e2e/policy-surface.mjs` reads `GET /api/v1/runs/{id}` after a real run settles and asserts
    the field is **omitted** rather than zeroed. The daemon's composed provider is the scripted one,
    whose terminal carries no usage block — deliberately, because it does not call a model and
    fabricating a consumption figure would put a measurement nobody made into a durable record. So the
    journey can only reach the absent branch, and the *present* branch (the sum appearing and equalling
    its inputs) is asserted by `a_run_read_reports_the_usage_its_calls_recorded`, which drives a real
    router over a real migrated store with a provider that does report usage. **Two checks, two
    branches; neither alone covers it.** A journey check that silently took the absent branch and
    called it passing is the shape this note exists to prevent.
  - **`clippy`'s line bound fired and the extraction was the right fix**, as in rounds 102 and 103: the
    fixture crossed 100 lines once it gained a variant, and `fixture_provider` is now one concern
    (built from one shared answer script) rather than a `match` buried in state assembly.
  - **Still open:** `budget_is_verifiable` remains **uncalled**, so a ceiling the provider left
    unmeasurable is still not *reported* as unverified. The read path that makes it reportable now
    exists, but the field to carry the verdict does not, and a field that is always `true` in this build
    would be a constant in a durable payload.
- [x] `BRN-059` Publish the consumption ceilings on the run read, so the client-visible
  `run.budget_output_tokens_exceeded` code names a number the client can see.
  Evidence: **falsified by projecting a wrong value** (`max_output_tokens.map(|_| 1)`), which fails
  `a_run_read_reports_the_ceilings_its_error_codes_name` with the published body showing
  `"limits":{"max_output_tokens":1}` against the 4096 the budget applies. Restored.
  - **The defect is `BRN-053`/`BRN-054` one field over, found by asking what a client can inspect.**
    `BRN-053` added `deadline_at` so `run.deadline_exceeded` was explicable; `BRN-054` recorded the
    usage that breached a ceiling so a reader had a number to compare — and the *ceiling itself* was
    never on any surface. `StoredRun.budget` has always carried it (`budget_for` applies
    `DEFAULT_MAX_OUTPUT_TOKENS`), and `RunView` rendered none of it, so a client holding the code and
    the usage still could not tell what limit it crossed.
  - **The ceiling is also not client-settable**, which makes the omission worse: `CreateRunRequest`
    carries `runtime`, `input`, `model_policy`, and `retry` — no budget. So the daemon chose the
    number, enforced it, refused the run by naming it, and published it nowhere.
  - `RunLimitsView` is a **separate type from `RunUsageView`** rather than one block, because they
    answer different questions (what was consumed vs. what was allowed) and a single type would invite
    a reader to compare fields across the two. Both are omitted when unset — an unset ceiling and a
    ceiling of zero are different facts, and the cost ceiling is unset in this build because there is
    no pricing catalogue.
  - 1515 tests (+1), docs synced in the same change.
  - **Not closed by this slice:** a client still cannot *set* the ceilings, so the numbers it reads are
    always the daemon's defaults or a policy's. That is a request-side gap rather than a read-side one.
  - **Correction to what `BRN-058` said about the enforcement path, because it was an overclaim.**
    `BRN-058` recorded "the enforcement path still judges one call rather than the run total" as a
    correctness gap. Tracing `ask_model` while writing this round: its loop retries **only**
    `FailureSite::BeforeAcceptance` failures, and `fail_open` returns `Retryable` only for a
    `Transient`/non-cancelled/non-timed-out error — so **at most one call per run can report usage**,
    and `exceeded_by` on that one call *is* the run total today. The gap is **latent, not observable**:
    it becomes real the moment a second reporting call exists in one run (a tool loop, a provider
    fallback, a multi-turn continuation), and it would then silently bound one call while claiming to
    bound the run. The invariant is now **pinned by a test** rather than described:
    `a_retry_with_no_budget_delay_still_completes_and_costs_nothing` already drove two calls, so it now
    loads the run's calls and asserts none reported usage — a future change that lets a retry follow an
    accepted call, or adds a tool loop, fails it and is told to move `exceeded_by` onto `Usage::summed`.
    Falsified by asserting `reporting == 1` (`left: 0, right: 1`). `Usage::summed` + `load_run_calls`
    are the read side of closing it; the check should move onto the accumulated figure **when** a second
    reporting call becomes reachable, not before — a run-level check that can only ever see one call is
    the same value with more machinery.
  - **Named, not built: a client still cannot set the consumption ceilings.** `CreateRunRequest` carries
    `runtime`, `input`, `model_policy`, and `retry`, and no budget, so the numbers a client reads are
    always the daemon's default or a policy's. Adding one is **not** mechanical: a caller-supplied
    ceiling must be **clamped by the effective policy**, or a client could widen a token or cost bound
    the policy set — the same "a submission must not widen what is in force" rule `PolicyService::put`
    enforces for data rules. `PolicyRules` models locality, retention, sensitivity, and residency, not
    token or cost ceilings, so there is no merge to reuse and the interaction needs a decision (an ADR,
    or a `RunBudget` merge that takes the minimum of the caller's and the policy's). Until then the
    contract specifies no such field, and this is recorded rather than guessed at.
- [ ] `TLS-010` Add MCP negotiation, auth, cancellation, malformed payload, and
  conformance/Inspector tests.
- [x] `BRN-060` Close the second way a validated value is bypassed on the way in, which round 90's
  `#[serde(transparent)]` sweep could not see: `PreviewItem` (a struct with **public fields** and a
  derived impl) and the approval summary (which was a bare `String` whose bound, `MAX_SUMMARY_BYTES`,
  was **checked nowhere in production**). Both are rendered inside the consent prompt a user reads to
  decide, and the preview is read back from `preview_json` on every approval load — so the reader was a
  path an unvalidated value could take into that prompt.
  Evidence: `crates/jarvis-domain/src/tool/wire_validation_tests.rs` (two new tests),
  `crates/jarvis-infrastructure/src/storage/approval_repository_tests.rs` (two corruption tests), and
  `ApprovalSummary` in `jarvis_domain::tool::approval`.
  - **The rule was never about the attribute.** Round 90 swept for `#[serde(transparent)]` because that
    was the mechanism it found, so it recorded nine instances and concluded the defect was confined to
    `jarvis-domain`'s transparent newtypes. But the *defect* is "a type with a validating constructor or a
    documented bound has no path from the wire through that check", and a derived `Deserialize` on a
    plain struct, or a `String` field with a free-function rule beside it, are two more ways to have it.
    Both are now closed for the approval module.
  - **`PreviewItem`'s fields are private now.** They were `pub`, so a caller could pass a good pair to
    `new` and then **assign** an empty or control-bearing half — the same bypass as the derived
    `Deserialize`, reached through a field rather than through the wire. `key()`/`value()` are the
    accessors, and `Deserialize` goes through `new`. There was exactly one production reader
    (`http::approval::preview_rows`), so the change is contained.
  - **The summary is now a type** (`ApprovalSummary`) rather than a `String`. `is_usable_summary` existed
    and was **referenced only by its own tests** — its doc said "the summary's bound is checked where a
    request is built" and no request-building path checked it — and the adapter read the column with
    `text(row, "summary")?` and no check at all. A newtype makes the value constructible only through its
    bound, so the two places that carry a summary (the request parts and the durable record) cannot skip
    it, and the **reader validates a stored row** rather than trusting the column.
  - **`ApprovalPreview`'s count bound now holds on the way in.** The transparent derive wrapped the vector
    directly, so a document with more than `MAX_PREVIEW_ITEMS` rows was accepted; the storage read went
    through `new` (which hides why this was invisible), but the `Deserialize` was still a bypass and had
    no test.
  - **3 mutations, all killed.** Restoring a direct-wrap `Deserialize` for `PreviewItem` fails
    `an_approval_preview_cannot_arrive_over_the_wire_unvalidated` (`assertion failed:
    !accepts::<PreviewItem>(r#"{"key":"","value":"x"}"#)`). A second run mutating `ApprovalPreview` and
    `ApprovalSummary` the same way fails both of their tests, naming the empty summary — and the two
    failures are distinct test names, so neither was masked by the other. A first attempt at the
    `PreviewItem` mutation left the derive in place and hit `E0119` (conflicting impls), which proves
    nothing; it was discarded and redone as a compiling direct wrap.
  - **Also corrected in passing:** a stale doc block on `assurance_of` in
    `jarvis_application::approval_service` that still said "the required level is always `Standard`" and
    carried two `# Errors` sections — both made false by the step-up rule the previous round added.
  - 1567 workspace tests (+4). All gates green (fmt, clippy, test, doc), both docs gates green, all five
    journeys green. **DO NOT COMMIT.**
  - **Not done, and named:** this closes the approval module. The same question — "does this type have a
    validating constructor and a derived or absent deserializer?" — has not been swept across the *rest*
    of the crate for non-transparent structs, and a bare field with a free-function rule beside it (the
    summary's shape) is the pattern least likely to be found by an attribute search.
- [x] `BRN-061` Widen the `BRN-060` sweep from transparent newtypes to **every type whose constructor
  returns `Result<Self>`**, which is how the same class is reached when the type is an ordinary struct
  with named fields. Nine instances in `jarvis-domain` were deriving `Deserialize` while their
  constructors enforced rules the derived impl never called — including `ToolDefinition`, whose own doc
  claimed "constructed only through `new`, so every instance satisfies the cross-field rules".
  Evidence: `crates/jarvis-domain/src/tool/wire_validation_tests.rs` (10 new tests, one per instance plus
  the positive control), and a validating `Deserialize` on each type.
  - **The sweep was calibrated before it was trusted, and the calibration is why it found anything.**
    The first version of the finder reported **zero** candidates — which is exactly the false-negative
    direction this project has recorded (`debugging.md`: under-reporting reads as a clean result). It had
    two bugs: a window too short to see a multi-line signature, and `Result<` anywhere in the signature
    rather than in the **return type**, which is the actual validation point. With stage counts printed,
    the tuned version reads `268 pub structs → 43 with a fallible constructor → 12 deriving Deserialize`.
    A finder that cannot be shown to have found anything is not evidence; the stage counts are.
  - **Nine are real and now go through their constructors.** `ToolSource` (inside `ToolIdentity`, read
    from `tool_identity_json` on every approval and ledger load), `DataClasses` (the cross-field
    "output never below input" rule), `ExecutionDefaults` (zero timeout / zero attempts), `PathGrant`
    (a grant conferring nothing, or listing a mode twice), `ReservationKey` (**an empty idempotency key
    makes every unkeyed call collide with every other** — the exact duplicate a reservation exists to
    prevent), `RetryPolicy` (**the one that is actually stored**, inside `agent_runs.budget_json`),
    `ToolResultBody` (the total byte bound), `ToolCallIntent` (capability and reason-summary bounds), and
    `ToolDefinition` (five cross-field rules, reached through `DiscoveredCatalog`).
  - **⚠ Three were deliberately LEFT derived, and the reason is a property of the read path rather than
    laziness.** `ReleaseManifest`, `DiscoveryFile`, and `SignatureEnvelope` all have a fallible
    constructor, which is what put them on the list — but each is read **only** through a `parse` that
    deserializes and then calls a validator returning a **typed** error (`ReleaseError`,
    `DiscoveryReject`). A validating `Deserialize` can only report through `serde`'s single opaque error,
    so it would collapse those variants: `UnsupportedSchemaVersion { found: 2 }` became
    `ManifestMalformed`, turning "this release is for a newer schema" into "the file is not valid JSON".
    **A test caught it** — `a_manifest_with_an_unsupported_schema_version_is_refused` failed the moment
    the change was made — which is the round's most useful measurement. The class needs **both** halves:
    a validating constructor *and* an unvalidated path to it. Where the only path is `parse`, the derived
    impl is not reachable with an invalid value, and "fixing" it is a regression.
  - **`SignatureEnvelope` is the `CredentialVerifier` case again**: its `parse` only checks a byte size,
    which a `Deserializer` cannot enforce, and its real checks (the algorithm comparison, the base64url
    decode) happen at the point of use in `verify_manifest`. No field-level invariant, so no custom impl
    — the same distinction round 90 recorded, now asserted rather than assumed.
  - **10 mutations, all killed, each in isolation with the other fixes in place so none was masked by a
    sibling.** Two batches of direct wraps, each failing **distinct** test names
    (`a_tool_source_…`, `data_classes_…`, `execution_defaults_…`, `a_path_grant_…`,
    `a_retry_policy_…`, `a_reservation_key_…`, `a_tool_result_body_…`, `a_tool_call_intent_…`,
    `a_tool_definition_…`).
  - **Also corrected: a doc claim that was false.** `ToolDefinition`'s "constructed only through `new`"
    is now true, and the doc says when it was not; the `ToolSource` fix additionally documents that its
    owner is what makes `ACC-024` hold for a same-named tool.
  - 1577 workspace tests (+10). All gates green (fmt, clippy, test, doc). **DO NOT COMMIT.**
  - **Not done, and named:** the sweep covers `jarvis-domain` and the two `crates` outside it that had a
    candidate. It does **not** cover a bare field with a free-function rule beside it (`BRN-060`'s
    `summary` shape) in other modules, nor a `Deserialize` derived on a type with no constructor at all —
    both are forms no constructor-based finder can see. There is no registry-level consumer yet, so the
    `ToolDefinition` half is proven by a round trip and by the falsifications rather than by a stored
    catalog.
- [x] `BRN-062` Sweep for the complementary shape `BRN-060`'s note named but did not build — a rule that
  exists and is enforced by nobody — and for **doc claims that name a guard which does not exist**. Found
  one missing test, two false doc claims, and two duplicated predicates. **A doc that names a test is a
  checkable claim, and nothing in this repository was checking it.**
  Evidence: `every_operation_has_a_spelling_and_parses` (new, in `jarvis-domain`'s ledger tests), the
  corrected `LedgerOperation::ALL` doc, the corrected `assurance_of` doc, and the two delegations in
  `RunBudget::permits_step_at` / `RetryDecision::decide`.
  - **The finder was wrong twice, in the under-reporting direction, and both bugs were found by
    calibrating rather than by reading.** Stage 1 (every public bool-returning fn in a non-test file)
    reported 105 for a name-prefix pattern and 176 once widened to any `-> bool`. The **zero
    production-caller** test was first written as `refs <= 1`, which counts *test* references as
    references — so `is_granted`, with five test callers and no production caller, was invisible. Fixing
    it to `production reference count == 0` took the list from 4 to 21. Same lesson as rounds 124/125
    (`debugging.md`): **under-reporting reads as a clean result**, so the finder has to be calibrated.
  - **⚠ `LedgerOperation`'s doc named `every_operation_has_a_spelling_and_parses`, and no such test
    existed.** That matters more than a stale comment because the operation column **is** stored —
    `SqliteToolCallRepository` writes `operation.as_contract_str()` and reads it through
    `LedgerOperation::parse` — so a variant added to the enum without being added to `ALL`, or one whose
    `as_contract_str` disagreed with its serde form, would produce a value JARVIS writes and cannot read.
    That is precisely the defect `ToolCallState` had (two spellings, found only by asserting the
    serialized bytes). The test now asserts the set size, the round trip for each variant, and that the
    serde form and the spelled form agree for **every** variant.
  - **Falsified both halves, and each failure names the right thing.** Dropping `Read` from `ALL` fails
    with `the contract lists three operations: left: 2, right: 3`; changing `Execute`'s spelling to
    `EXECUTE` fails with `Execute must serialize to the spelling it parses from: left: "\"execute\"",
    right: "\"EXECUTE\""`. The second is the `ToolCallState` defect reintroduced deliberately, and the
    test names it rather than reporting a set-size mismatch.
  - **A second false doc claim:** `assurance_of` cited `a_guest_is_refused_before_the_store_is_read`; <!-- citation-history -->
    the test is `a_guest_cannot_decide_and_is_refused_before_the_record_is_read`. Corrected rather than
    left, because a reader following the citation would not find the test and would conclude the
    guarantee is unproven — the opposite of what the sentence intends. **`BRN-064` later found this
    same sentence cited a name defined nowhere in the workspace**, and only because the correction
    wrote the stale name in a shape a checker could read: a citation in backticks is a claim whether a
    sentence says it is current or superseded, so the checker reports it either way and a reader has to
    judge. The four other stale names `BRN-064` reported are all like this one — inside sentences that
    say "renamed from" or "was named" — so they are correct history rather than defects.
  - **Two predicates had no production caller because a caller restated them.** `BudgetStatus::is_permitted`
    was defined as `!matches!(self, Expired)` while `RunBudget::permits_step_at` wrote the *same*
    expression inline, and `FailureClass::is_transient` had **zero** references anywhere while
    `RetryDecision::decide` compared `class == FailureClass::Permanent` — the complement. Both are the
    "two spellings of one rule" defect: editing one leaves the other silently disagreeing. Each call site
    now delegates to the named predicate, so the rule has one definition; `is_transient` gains the
    production caller that makes it more than documentation.
  - **Reviewed and deliberately left alone, each with a reason rather than a shrug:** the tool-fabric
    predicates (`permits_dispatch`, `is_granted`, `is_settled_duplicate`, `needs_approval`, `is_denied`,
    `has_effect`, `is_same_tool_as`, `is_external`, `is_replacement_of`, `requires_caller_key`,
    `is_consumed_on_use`, `can_advance`, `has_artifacts`, `is_shape`) have no consumer because the
    fabric's HTTP surface does not exist and their own notes say so — a *known* gap, not a
    documented-as-enforced claim. `RetryDecision::is_retry` is a public accessor on a public type used
    only by tests. `ControllerError::is_unimplemented` is public-API surface. `is_client_visible` **says
    in its own doc** that it has no caller and why, with a test that keeps it true.
  - 1578 workspace tests (+1). All gates green (fmt, clippy, test, doc). **DO NOT COMMIT.**
  - **Not done, and named:** the sweep covers public `-> bool` predicates in non-test files. It does not
    cover a private predicate with no caller, a `-> Option<T>` or `-> Result` accessor with no reader, or
    a doc claim naming a *module*, *field*, or *constant* that does not exist (only `fn` names were
    checked). The doc-claim check found two of nineteen; the other seventeen were verified by hand as
    existing, so the check is not yet a gate.
- [x] `BRN-063` Sweep for the **accessor** form of the same class — a public method with a producer and
  no consumer — which `BRN-062`'s note named as uncovered. Found **four false doc claims** about callers
  that do not exist, one of them a security-shaped claim, and one fail-open default that no test caught.
  Evidence: the corrected docs in `jarvis_infrastructure::tool_schema::confirms`,
  `jarvis_domain::tool::registry` (`all_unfiltered`, `registered_source_kinds`),
  `jarvis_domain::tool::identity` (`SourceKind` and `is_external`), and `jarvis_domain::tool::ledger`
  (`possibly_effecting_without_outcome`); plus `SourceKind::ALL` and the strengthened
  `source_kind_classifies_externality_deliberately`.
  - **The finder was wrong a third time, again under-reporting.** It counted production references as
    `name(` — a CALL — so it missed `map(RetainedItem::to_input_item)`, where the method is passed as a
    function **value** with no parentheses. That is a production caller, and the first run reported a live
    function as dead. Adding the `::name` path form took the list from 44 to 38 and removed the false
    positive. **Every sweep this session has needed its finder corrected before its output meant
    anything** (`debugging.md`): calibrate on a case you know has hits, and print stage counts.
  - **⚠ The claim with teeth was `ToolSchema::confirms`.** Its doc read "**this is what stops the
    validator from being a component nothing consults**" — and nothing consults it. `ToolSchema::parse`
    and `ToolSchema::validate` have no production caller either; the whole validator is reachable from
    tests and the module's own example. `TLS-003`'s note had already recorded that gap ("no adapter stores
    a schema and no request reaches the validator"), so the doc was claiming the opposite of the recorded
    state. Corrected to say what the check is *for* and that it has no caller **yet**, keeping the
    `ACC-024` rationale intact because the check is right and only uninvoked.
  - **Three more "used by" claims, all false:** `registry.all_unfiltered` ("this is what a discovery
    answer for one scope is computed *from*" — `catalog_for` takes its tools as an argument, and the
    production call that would pass them does not exist), `registered_source_kinds` ("used by an operator
    view" — there is no registry operator view at all), `SourceKind::is_external` ("used by policy" — no
    policy module calls it), and `possibly_effecting_without_outcome` ("what makes the recovery able to
    find the calls `ACC-025` is about" — recovery reads the **SQL** scans in
    `jarvis_infrastructure::storage::tool_call_repository`, not this in-memory method).
  - **The finding behind the security-shaped one: `is_external` was not enumerable, so an added source
    kind silently defaulted to TRUSTED.** The predicate is a `matches!` over three named variants, so a
    sixth `SourceKind` compiles with no answer and reads as *not external* — the fail-open direction,
    where a new remote source is treated as native. `source_kind_classifies_externality_deliberately`
    named the five kinds individually, so it kept passing when a sixth was added. Fix: `SourceKind::ALL`,
    the same enumeration `ToolCallState::ALL` and `LedgerOperation::ALL` use, and the test now builds its
    list from `ALL` and compares it to a hand-written table. **Falsified** by adding a variant: the test
    fails printing `(WasmRuntime, false)` — naming the fail-open default rather than reporting a
    mismatch. Its own doc's promise ("a source kind added later must be classified deliberately") is now
    enforced rather than hoped for.
  - **Reviewed and left alone, each with a reason:** builder/consumer methods on `ScriptedProvider`
    (`also_serving`, `emit_raw`, `interrupt`, `fail_first_opens`), `WaitingWrite::waiting_on`,
    `RunBudget::routed_model` (`route` is also read through the budget's own accessors), the
    diagnostics/install/release accessor sets (operator-rendered, and `render()` is called),
    `Observed::redactor`/`log_directory` (used by the `Debug` impl and the CLI), and the tool-fabric
    accessors whose notes already say the fabric has no surface.
  - 1578 workspace tests (unchanged — one test strengthened, none added). All gates green (fmt, clippy,
    test, doc). **DO NOT COMMIT.**
  - **Not done, and named:** the sweep covers `pub fn (&self…) -> T` accessors in non-test files. It does
    not cover private accessors, trait-impl methods (excluded deliberately: they satisfy a trait), or
    `pub const fn` on non-`self` values. The doc-claim checker now covers `fn` names; it still does not
    check claims naming a module, field, or constant, and it is not yet a gate — 4 of the 5 claims this
    round corrected were about *callers* rather than names, so a name-existence check would not have
    found them either.
- [x] `BRN-064` Make the test-citation check a **permanent gate** rather than a hand sweep, and fix the
  one stale citation it found in a contract document. `BRN-062` found this class by hand; nothing stopped
  it recurring, and a contract's test table is exactly where a reader looks to decide whether a rule is
  proven.
  Evidence: `unresolvedTestCitations` + `validateCitations` in `scripts/validate-docs.mjs` (wired into
  `main`), 3 fail-closed tests in `scripts/validate-docs.test.mjs`, the corrected citation in
  `docs/contracts/model-data-policy.md`, four `<!-- citation-history -->` markers in `TODO.md`, and the
  new **Test Citation Check** section in `docs/operations/ci-gates.md`.
  - **⚠ The contract citation was the defect: `docs/contracts/model-data-policy.md` cited
    `a_grant_needing_step_up_is_refused_at_standard_assurance`, which no longer exists.** The test is <!-- citation-history -->
    `a_grant_needing_step_up_is_refused_for_a_standard_context` — renamed when it was rewritten to vary
    the *server-derived* assurance rather than a body field, which is the stronger test. A reader
    following the citation into the table's "Falsified by" column would find nothing and conclude the
    step-up rule is unproven, the opposite of the table's intent. The corrected name resolves.
  - **The checker validates against the sources, not against a list.** It collects every `fn` in
    `crates/` and `apps/` and every test-shaped backticked identifier in the Markdown corpus, and
    compares the two — the `BRN-016` lesson ("the document against itself") applied to citations. A
    hand-maintained allowlist would be a second thing to drift.
  - **Four stale citations are correct history rather than defects, and the marker is how the checker
    tells.** `BRN-062`'s own record cites `a_guest_is_refused_before_the_store_is_read` (naming the stale <!-- citation-history -->
    value in order to correct it), and three TODO entries cite names their tests were renamed *away from*,
    in sentences that say so. A prose heuristic that tried to detect "this sentence is about the past"
    would fail in the dangerous direction — a citation that silently stops being flagged reads as a clean
    corpus — so the exclusion is an explicit per-line `<!-- citation-history -->` marker: an HTML comment,
    invisible when rendered, greppable, and a deliberate statement that the name is history.
  - **The finder was wrong once more, in the OVER-reporting direction, and calibrating caught it.** The
    first version collected type names, function names, constants, and enum variants but **not struct
    fields**, so twelve `Struct::field` paths (`RunBudget::max_output_tokens`,
    `PolicyRequest::action_digest`) read as unresolved. Over-reporting is the direction that reads as a
    work list, so it was fixed before the result meant anything. With fields collected, all **646**
    backticked `Type::item` paths in doc comments resolve — a clean result that only now means something,
    because the same-looking finding had a different cause the first time.
  - **Falsified end to end three ways.** Removing a `<!-- citation-history -->` marker makes the
    validator exit `1` naming `a_guest_is_refused_before_the_store_is_read (cited at TODO.md:5034)`;
    replacing a real contract-table citation with `a_test_that_does_not_exist_anywhere_at_all` makes it <!-- citation-history -->
    exit `1` naming `docs/contracts/model-data-policy.md:259`; restoring each returns it to `0`. The unit
    tests additionally drive the pure function with a **resolved** citation, which is the positive control
    that stops "report everything" from satisfying the check.
  - **Not covered, and named:** it matches a test-shaped name (article prefix plus ten characters), so a
    shortened name, a name in prose without backticks, a path, or a claim about a *module* or *constant*
    is not caught — the same boundary `BRN-062` recorded. And a citation that **resolves** is not the same
    as a citation that is **true**: `BRN-063` found four claims about callers that named a real function
    the caller did not exist for, which this check cannot see at all.
  - 23 validator tests (+3). 1578 workspace tests (unchanged). All gates green (fmt, clippy, test, doc,
    both docs gates). **DO NOT COMMIT.**
- [x] `BRN-065` Enforce the **error-code namespace rule** at the emission boundary and reconcile the
  contract's own list with the codes the surface emits. The rule was documented as normative and
  enforced nowhere: three things were true at once and no check covered any of them.
  Evidence: `jarvis_protocol::CODE_NAMESPACES` (16) + `INTERNAL_CODE` + `is_owned_code`, the namespace
  check inside `ErrorEnvelope::new`, the widened `ErrorCode::NAMESPACES` in `jarvis-domain`, two new
  `jarvis-infrastructure` guards (a cross-crate survival check and a set-equality check), two new
  `jarvis-protocol` unit tests, the corrected normative list in `docs/contracts/common-conventions.md`.
  - **The three facts.** `docs/contracts/common-conventions.md` said only nine namespaces are JARVIS's
    and that a code outside them "is replaced by `jarvis.internal` at the emission boundary". (1) That
    rule had **no production caller**: `ErrorCode` (`jarvis-domain/src/error.rs`) is referenced from
    nothing outside its own module and tests. (2) The boundary real responses pass through —
    `ErrorEnvelope::new` — **validated nothing**: it took `impl Into<String>` and stored it, so it could
    not consult `ErrorCode` even by accident. (3) The surface emits **seven namespaces the list omits**:
    `request`, `api`, `auth`, `resource`, `idempotency`, `service`, `internal`. The contract's own
    Minimum-codes table, twenty lines below the list, **names five of those seven** — so the "normative"
    list was not a superset of the table beside it. The two documents contradicted each other in a file
    the docs validator reads but cannot judge.
  - **The direction that was almost lost.** The tempting reading is "the list is too long, trim it".
    It is the opposite: the list was missing namespaces, and `ErrorCode` had already been corrected once
    for exactly this. `BRN-062`'s round found that a `jarvis.`-only rule would have rewritten the tool
    contract's sixteen `tool.*` codes and the approval contract's twelve `approval.*` codes to
    `jarvis.internal`, collapsing every code a client branches on. Wiring that same `ErrorCode` into the
    boundary **without** widening it would have done exactly that to `request.`, `api.`, and the rest —
    so the fix is not "enforce the rule as written", it is "make the rule and the codes agree, then
    enforce it". A test that had asserted the enforcement without the widening would have passed while
    the daemon shipped `jarvis.internal` for every `409` and `429`.
  - **Why the enforcement is in `jarvis-protocol` and not in `jarvis-domain`.** The list is wire
    vocabulary, and `jarvis-protocol` depends on no other JARVIS crate — so it is the one place a
    boundary can consult without depending inward, which `jarvis-domain` cannot do (it has no internal
    dependency, and the emission type is `String`). `jarvis-domain`'s set is the **mirror**, not the
    enforcement point, and its doc now says so; the two are held equal by an executable check.
  - **The set-equality check is not decoration.** `BRN-065` is the failure of "two spellings of one
    rule": a documented set living apart from the enforced one. `the_domain_and_protocol_namespace_sets_are_the_same_rule`
    makes the second spelling impossible, and it also asserts the replacement code is **in the set it
    enforces** — a replacement no namespace owned would itself be rewritten, turning the fail-closed
    direction into a rewrite to something else.
  - **The cross-crate guard exists because neither crate can see both ends.** `jarvis-domain` cannot see
    the wire and `jarvis-protocol` cannot see the domain, so a divergence is invisible from inside
    either. `jarvis-infrastructure` depends on both, which is why
    `every_code_this_surface_produces_survives_the_emission_boundary` lives there: it runs the surface's
    own code scan (`production_codes`, reused rather than re-invented) against the real constructor and
    fails naming a code the boundary would rewrite. Both halves of the round are needed: enforcement
    without the survival check passes while a namespace is missing from the set; the survival check
    without enforcement passes while nothing is replaced.
  - **A struct literal would bypass the constructor, so the check is on the construction sites.** The
    wire shape is public (a client parses it), so `ErrorResponse { .. }` is reachable and skips the
    namespace check. Rather than claim otherwise in a doc comment — the defect this round is about — the
    survival test asserts the crate has **exactly one** construction site, the constructor, so adding a
    second fails. Its own doc comment names the check, so the claim and the guard cannot drift.
  - **Falsified in both directions.** Removing `"api"` from the protocol set makes the survival check
    fail `api.forwarded_header_not_allowed is produced by this surface but the emission boundary rewrote
    it` (the code is named, not the namespace) and the set-equality check fail
    `the domain and protocol namespace sets must be identical`; disabling the check inside
    `ErrorEnvelope::new` makes `a_code_outside_the_owned_namespaces_is_replaced` fail
    `acme.thing must not be forwarded as a JARVIS code`. Restoring each returns the gate to `0`. The
    protocol test's namespace table is compared against `CODE_NAMESPACES` **as a set in both
    directions**, so a namespace added to the const without a sample — the one change that could slip
    through — is a failure.
  - **One claim of mine was wrong and the check is what disproved it.** I read
    `error_code_rejects_a_non_namespaced_value` as asserting the wrong answer for `toolbox.x`, on the
    reading that `starts_with` is a prefix test. It is a prefix test, and `starts_with(ns) && bytes[len]
    == b'.'` requires the `.` at exactly that position, so `toolbox.x` is correctly rejected — which the
    test, run, confirms. Recorded because the finding was in my reading, not in the code, and running
    the one test settles it in seconds.
  - **Not done, and named.** `ErrorCode` still has no *production* caller: the boundary is
    `String`-based, so it cannot be routed through the type. It is now the domain-side mirror with an
    executable tie to the enforced set, which is the honest description rather than the "presentation
    type for boundaries" the doc claimed while nothing used it. The domain test asserts the domain half
    only; `jarvis-infrastructure` asserts the pair. And the surface scan sees only string **literals**
    written in `src/http/` — codes carried on error types (`run.*`, `storage.*`, `idempotency.conflict`)
    are invisible to it and are covered by `CODES_CARRIED_BY_ERROR_TYPES`, which remains a
    hand-maintained list.
  - 1582 workspace tests (+4). 23 validator tests (unchanged). 188 TODO IDs (+1). All gates green (fmt,
    clippy, test, doc, both docs gates). **DO NOT COMMIT.**
- [x] `BRN-066` Complete the `BRN-065` inverse sweep: **a rule written as a function that nothing
  calls**. `BRN-065` found the shape on a `pub` method (`ErrorCode::new`); this round looked for it in
  free functions, then in the `pub` methods `BRN-065`'s scope had deliberately skipped, and calibrated
  the finder against a fixture before trusting it.
  Evidence: `join_under`, `verify_within_profile`, and `is_safe_component` removed from
  `crates/jarvis-infrastructure/src/paths.rs`; the two error variants they constructed
  (`UnsafePathComponent`, `PathOutsideProfile`) removed from `error.rs`; the duplicated `normalize`
  deleted from `install/mod.rs`, which now uses the `pub(crate)` one in `paths.rs`.
  - **The finding: a complete, tested, documented path-safety chain with no caller in any crate.**
    `verify_within_profile`'s doc said it "prevents a crafted name from redirecting durable state
    elsewhere"; `join_under`'s refused "empty, `.`, `..`, absolute, rooted, or ... a separator or drive
    prefix". Both were `pub`, both had thorough unit tests, and both were called by nothing outside
    those tests. This is worse than ordinary dead code: a reader (or an audit) that finds a tested
    containment function reasonably concludes traversal is handled, and it is not handled *there*.
  - **The finder was wrong three times and the fixture is what caught it.** (1) `tests.rs` modules carry
    `#[cfg(test)]` on the `mod tests;` line in the *parent*, so the file body has no marker and the
    whole file read as production — 383 hits, nearly all test names. (2) `text.indexOf("#[cfg(test)]")`
    and truncating there is wrong when the test module sits **mid-file**, as `service/mod.rs` has: it
    hid every definition and call site after it, so `xml_escape` (called three times) read as uncalled.
    (3) counting a name's occurrences including **comments** meant a function merely *named in a doc
    comment* counted as referenced — which is the dangerous direction, because an uncalled rule usually
    has a doc comment explaining it, so the better documented it is, the more invisible it becomes. A
    crafted fixture with known-uncalled and known-called functions is what exposed (3); the positive
    control (a function with one caller) did not, because nothing in the real tree was named in a
    comment in a way that masked a *real* defect — only the fixture's own header comment did.
  - **The honest result is a negative one, said plainly.** After calibration the free-function sweep
    over 121 modules and 423 definitions found **zero** uncalled rules. That is a result, not a failure:
    it retires the class for free functions in this tree. The 102 uncalled *methods* it then reported are
    almost all legitimate test seams, and the ones that are not — the in-memory `ToolCallLedger`
    (constructed nowhere outside tests), `ToolRegistry`'s query methods, `ToolDiscoveryCache`'s
    invalidation, `ApprovalRecord`'s transition predicates — already carry docs corrected by `BRN-062`
    and `BRN-063` that say exactly that. So the class was swept, not merely sampled.
  - **Why the dead chain was deleted rather than wired up.** `BRN-065`'s lesson was "do not enforce a
    rule that is wrong". Here the rule is fine but the *need* is absent: no HTTP route takes a
    filesystem path (`Path<String>` carries ids), diagnostics derives its paths from `ProfilePaths`, and
    the one place untrusted names do become filesystem access — the release manifest's artifact names —
    is already guarded by `is_plain_file_name` at the join site. Containment is enforced where it is
    needed by `InstallLayout::contains`, `InstallLayout::is_inside_profile`, and the domain's
    `PathGrant`. Keeping a fourth implementation nobody consulted was the defect; deleting it is the fix.
  - **Every "is it live?" claim was checked by finding the call site, and three of my own were wrong
    before I did.** I wrote that `ProfilePaths::contains` was live (its only caller is
    `profile/mod.rs`'s test), that `InstallLayout::contains`'s lexical-prefix case was untested (it is
    asserted in `a_portable_layout_keeps_every_path_under_its_root`), and that `is_plain_file_name` had
    only a positive test (its negative matrix runs through `manifest.validate()` in `release/tests.rs`).
    Each correction came from grepping for the caller rather than reading the name — the same
    distinction `BRN-065` recorded, and it was needed three more times in one round.
  - **No coverage was lost, and that is demonstrated rather than asserted.** `root-escape rejection`
    is a checked box in `docs/research/integrations/rust-foundation.md`, which is why the deletion had
    to be justified: `crates/jarvis-domain/src/tool/path_grant_tests.rs` covers lexical containment
    ("a sibling that shares a string prefix is not") for the *production* path-authorization rule, and
    `install/tests.rs`, `diagnostics/tests.rs`, and `diagnostics/archive.rs` cover it for their own
    surfaces. The deleted tests were the only coverage of a function nothing called.
  - **Not done, and named.** The dead-method finder stops at "docs that claim a production role" and
    still produces 15 loose matches (ordinary words like `every`, `always`, `never` in ordinary
    rationale), so it is a reading aid rather than a gate, and it is not checked in. It also cannot see
    a dead rule expressed as a *macro*, a *trait default method*, or a `const fn` in an `impl` reached
    only through a generic bound.
  - 1579 workspace tests (−3: the deleted dead tests). 23 validator tests (unchanged). All gates green
    (fmt, clippy, test, doc, both docs gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-067` Make the code table's completeness check able to see **the codes that reach a client
  through an application service error's `code()`**, list the fourteen it was hiding, and correct the
  false claim in `BRN-065`'s own contract sentence. Found by reviewing `BRN-065` rather than by a new
  sweep, because the defect was in that round's reasoning: it added a boundary that enforces *whose*
  code a value is, and then wrote as though that also enforced *whether anyone had written the code
  down*.
  Evidence: the new `every_code_an_application_service_error_carries_is_in_the_contract_table` test
  (which scans `approval_service.rs`, `policy_service.rs`, and `run_service.rs` for owned codes and
  compares them against the table), fourteen added rows in `docs/contracts/local-control-api.md`, the
  corrected completeness paragraph in the same document, and `jarvis.context_candidates_unbounded`
  documented in `docs/contracts/model-data-policy.md`.
  - **The defect: the table claimed to be complete and was missing fourteen codes.** The surface maps
    three application error types onto the envelope through `error.code()` —
    `ApprovalServiceError`, `PolicyServiceError`, and `RunServiceError` — and their codes appear as
    literals in `jarvis-application`, not in the surface, so the surface's own literal scan finds none
    of them. `CODES_CARRIED_BY_ERROR_TYPES` was the compensation, and it was **a hand-written
    two-item list** (`idempotency.conflict`, `resource.version_conflict`) against three `code()`
    implementations returning far more. Thirteen of the produced codes were absent from the table.
  - **One of them was in no document anywhere in the repository.**
    `jarvis.context_candidates_unbounded` is a `code()` value on both `PolicyServiceError` and
    `RunServiceError` — the route selector refusing to *examine* its input because more candidates
    were offered than `jarvis_domain::context::budget::MAX_CANDIDATES` (ten thousand) allows. It
    reaches a client as a **`403` on create-run** (the same code is a `400` on policy evaluate) and
    was named in zero Markdown files. A client meeting it had nothing to read. It now has a row and a
    paragraph explaining why it is `jarvis.*` rather than folded into `model.policy_unsatisfied` —
    folding it would tell a caller its policy excluded every model, sending it to edit a policy that
    was never read.
  - **The false claim was `BRN-065`'s, and it was false in the direction that hides this class.** That
    round added the sentence *"a code this table does not list cannot reach a client at all — it
    arrives as `jarvis.internal`"*. That is not what the boundary does: `ErrorEnvelope::new` replaces a
    code whose **namespace** it does not own, and every one of these fourteen codes is in an owned
    namespace, so they cross it untouched and a table row is never consulted. A reader trusting the
    sentence would conclude that a code missing from the table was unreachable and stop looking —
    which is precisely how the orphan survived `BRN-065`'s own review of this file.
  - **The guard scans the services' source rather than extending the hand list**, for the reason
    `BRN-064` recorded: a hand-maintained list is a second thing to drift, and three consecutive
    rounds found the drift had been in the direction of silence. It reads the same namespaced string
    literals the surface scan reads, over the three files that own the mappers, and asserts the result
    is a **subset of the table**. It also asserts it read all three files and found at least twenty
    codes, so a scan that silently stopped finding anything fails rather than passes — the vacuous-scan
    failure every fixture in this workspace exists to catch.
  - **Falsified in both directions.** Removing the `model.exception_required` row makes the guard fail
    naming exactly that code; introducing an unlisted code into `run_service.rs` makes it fail naming
    the new one, which proves the scan reads real source rather than a copied list. Restoring each
    returns the gate to `0`.
  - **A mutation nearly hid behind a stale build, and the lesson is a build one.** `Copy-Item -Force`
    restores a file's **original timestamp**, so restoring `run_service.rs` after a mutation left cargo
    reusing the artifact compiled from the mutated source — a test then failed against code that was no
    longer on disk, which reads as "my restore did not work" when in fact the file was already correct
    and the *binary* was stale. Fixed by touching the file; the durable fix is to verify a restore by
    content (`git diff` clean) **and** to expect one stale-artifact failure rather than re-editing a
    file that is already right.
  - **Not done, and named.** The scan is a literal scan over three files, so a code built by
    `format!` from parts, or carried in a fourth file added later, is still invisible — the same
    boundary the surface scan has. `PolicyServiceError`'s variant doc names a code
    (`jarvis.invalid_policy_layer`) that its `code()` can only receive from the caller, so the guard
    covers it through the `Contradictory { code }` arm's call sites rather than by construction. And
    the `409`/`403` statuses now recorded in the table for `approval.*` and `model.*` codes are taken
    from the mapping functions' arms, which no test yet compares against the table the way
    `resource.version_conflict` and `service.not_ready` are compared.
  - 1580 workspace tests (+1). 189 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both docs
    gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-068` Hold the code table's **other two columns** to the mappers that produce them — the
  follow-up `BRN-067` named and deliberately left undone. That round added the fourteen missing rows
  and took each status from the mapping function it read, but nothing compared the two, so a row
  could name a code correctly and state the wrong status or the wrong retryability with every gate
  green.
  Evidence: `the_service_codes_status_and_retryable_column_are_what_the_mappers_send` plus its
  `service_code_cases`, `run_service_code_cases`, `policy_and_approval_code_cases`, and
  `assert_mapper_row` helpers, the `status_cells` table reader, two `pub(crate)` test accessors
  (`policy_error_response_for_test`, `approval_error_response_for_test`), and the corrected
  `jarvis.context_candidates_unbounded` rows in `docs/contracts/local-control-api.md`.
  - **The test drives ten refusals through the three real mapping functions** and asserts three
    things at once for each: the status the mapper returned, the `retryable` flag in the body it
    sent, and *both columns of the table*. The three comparisons are the point — asserting the
    mapper's answer beside the document's is what makes this a cross-check rather than a document
    agreeing with itself, and it is the same shape the existing `resource.version_conflict` and
    `service.not_ready` checks already had for two codes.
  - **It caught a real inconsistency on its first run, in my own `BRN-067` edit.**
    `jarvis.context_candidates_unbounded` has **two** statuses — `403` on create-run, `400` on
    policy evaluate — and `BRN-067` put the `403` in a **row** and the `400` in **prose**. A status a
    client must handle belongs in the column a client reads, so the table now has both rows and the
    prose says why. This is the same defect class one layer on: not a missing row, but a present fact
    recorded where nothing can check it.
  - **The prose-first version would have looked correct to every existing gate.** The completeness
    test compares *codes*, so a code listed at one of its two statuses satisfies it; only driving the
    mapper that sends the other status shows the disagreement. That is why the follow-up was worth
    doing rather than declaring the table finished.
  - **The status reader returns a set per code, not one value.** A last-row-wins map would have
    silently dropped the `400` for the two-status code and failed against a document that was already
    right — so the fix had to be in the reader before the fix could be in the table, and the
    assertion became membership rather than equality. Worth recording because the obvious
    `BTreeMap<String, String>` is wrong here and would have produced a false failure that reads like
    a real one.
  - **Falsified in both directions.** Changing `model.policy_unsatisfied`'s row from `403` to `404`
    fails `the table must list model.policy_unsatisfied as 403 ... Some({"404"})`; flipping
    `approval.version_conflict`'s `Retryable` cell from `yes` to `no` fails `the table's Retryable
    column must agree with the mapper's own retryable()`. Restoring each returns the gate to `0`.
  - **Not done, and named.** The ten cases cover every code the three mappers produce **that the test
    names**, not every variant: `ApprovalServiceError::Storage`, the `Invalid`/`ScopeDenied` carried
    codes other than the ones exercised, and `RunServiceError`'s `NotFoundException` and storage arms
    are reached in this build only through a repository the fixture would have to be broken to
    produce, so they are unasserted here and are covered by their own `code()` tests in
    `jarvis-application`. The scan is still a literal scan, so a code built by `format!` from parts
    remains invisible — the boundary `BRN-067` also recorded.
  - 1581 workspace tests (+1). All gates green (fmt, clippy, test, doc, both docs gates), all four
    journeys pass. **DO NOT COMMIT.**
- [x] `BRN-069` Delete the hand list `BRN-067` left behind, now that a scan supersedes it — a loose
  end in my own previous round, found by asking what the list was still for.
  Evidence: `CODES_CARRIED_BY_ERROR_TYPES` removed; the extraction of `production_codes_from_services`
  (the scan) plus the `carried_codes`, `listed_codes`, `contract_document`, and `repository_root` test
  helpers; and the corrected doc comments on `production_codes` that had described the list as the
  mechanism.
  - **`BRN-067` added a scan and left the hand list it replaced in place.** Two mechanisms for one
    fact is exactly the shape this project keeps finding: they agree today and drift tomorrow, and
    `BRN-067`'s whole finding was that this list had drifted. The list also kept a **false claim**
    alive in its doc — that the two codes it named are "invisible to a scan" — while both are string
    literals in `approval_service.rs`/`policy_service.rs`/`run_service.rs`, which is precisely what
    the new scan reads. Verified by grepping the two codes' literals before deleting.
  - **The completeness test now unions two scans rather than a scan and a list.** `production_codes`
    reads this surface's own literals (correctly scoped — widening it would stop describing *this*
    surface) and `production_codes_from_services` reads the services' `code()` values; the union is
    what the table is compared against. So a newly carried code is covered without anyone remembering
    to add it, which was the defect, **fixed in both places rather than one**.
  - **Four duplication sites collapsed into shared helpers** while doing this: `repository_root`,
    `contract_document`, `listed_codes`, and `carried_codes`. Three tests were each re-deriving the
    repository root and re-reading the contract, which is a disagreement waiting to happen — a wrong
    number of `.parent()` calls reads as "the file is missing" rather than as a bug. The two vacuity
    guards (all three files read, at least twenty codes found) moved into `carried_codes` so a new
    caller cannot forget one.
  - **Falsified:** adding a new unlisted code to `approval_service.rs` makes
    `the_minimum_code_table_names_every_code_this_surface_produces` fail naming it — which it could
    not do before this round, because the test read literals in the surface and a two-item list. The
    scan-based version catches a service-carried code with no list at all.
  - **Not done, and named.** The scan is still a literal scan over three named files, so a **fourth**
    service file added later is invisible until it is added to the list in `production_codes_from_services`,
    and a code built by `format!` from parts remains invisible — both recorded by `BRN-067` and both
    unchanged here. The file list is itself now a hand-maintained list, which is the same shape one
    level up; replacing it would mean discovering application source files rather than naming them,
    and that is a deliberate step not taken in this round.
  - 1581 workspace tests (unchanged). All gates green (fmt, clippy, test, doc, both docs gates), all
    four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-070` Correct a **false reachability claim** my own `BRN-069` note made: that
  `RunServiceError::Controller(error).code()` yields `run.*` to a client. It does not — nothing
  constructs that variant — and three of the nine `run.*` codes its arms name are in no document.
  Evidence: the corrected variant doc in `crates/jarvis-application/src/run_service.rs`, the corrected
  500 arm in `crates/jarvis-infrastructure/src/http/runs.rs`, the corrected `production_codes` note in
  `crates/jarvis-infrastructure/src/http/mod.rs`, and
  `a_controller_failure_is_recorded_on_the_run_rather_than_reported_to_the_submitter` in
  `crates/jarvis-application/src/run_service/tests.rs`.
  - **The finding: a variant with a producer in the type system and none in the program.**
    `RunServiceError::Controller(ControllerError)` appears in exactly three places — the three match
    arms of its own `code()`, `retryable()`, and `message()`. There is no `From<ControllerError>` impl,
    no construction site in any crate, and **not even a test** builds it. So nine `run.*` codes reach a
    reader through three arms that cannot execute.
  - **It is unconstructed because the design says so, and the discard is explicit.** `spawn_run` states
    the reason where it drops the controller's return value: *"The outcome is deliberately not
    propagated: the run's terminal state is durable and is what a client reads, so a controller error is
    already recorded as the run's state rather than needing a channel to nowhere."* So a controller
    failure becomes the run's own `error_code` — which the contract does document as client-visible
    (`run.no_model_served`, `run.deadline_exceeded`, `run.budget_*`) — rather than an error returned to
    whoever submitted the run.
  - **Three of the nine are in no document at all.** `run.stream_interrupted`,
    `run.output_not_persisted`, and `run.clock_unavailable` are named in zero Markdown files. This is
    the same gap `BRN-067` closed for the table, in the other place a code can live: on a durable field
    read by the run resource rather than in an error envelope. They are reachable in the sense that
    matters — they can land on a run's stored row — so a client can meet a code with nothing to read.
  - **The correction is recorded rather than the variant deleted, and the reason is which decision is
    whose.** Removing it would change the run-creation API's error shape, which is a design choice for
    whoever next works on that route, not for a sweep about reachability. What this round owes a reader
    is the truth: the doc now says nothing constructs it and why, the 500 arm says its `code()` arm is
    unreachable, and the `production_codes` note that claimed it "would yield `run.*`" says what is
    actually so.
  - **The guard asserts the delegation, not a call-graph absence.** A test asserting "nothing constructs
    this" would fail the moment someone *correctly* wires it; the test instead pins what must hold either
    way — a controller failure reports the controller's own code, message, and retryability rather than
    a variant name — so it stays true if the variant becomes live and is the right failure if it is
    deleted. Naming it as the delegation is the honest form of a guard over an unreachable arm.
  - **Not done, and named.** The nine codes are **not** added to the control API's minimum-code table,
    because they do not arrive in an error envelope — they arrive on the run resource's `error_code`
    field, which that table's completeness test is scoped to the envelope and must not be widened to
    cover. Whether they need their own documented list is the same open question as the missing
    `run.stream_interrupted`; it is named here rather than resolved, because inventing a second table
    is a contract change and this round was a correction.
  - 1582 workspace tests (+1). 192 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both docs
    gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-071` Close the gap `BRN-069` named — extend the code scan to the fourth producer,
  `repository/mod.rs` — and list the six `storage.*` codes it immediately found missing, one of which
  the contract itself called unlisted while the surface was producing it.
  Evidence: `repository/mod.rs` added to `production_codes_from_services` with the file-count guard
  raised to 4, six `storage.*` rows and a corrected paragraph in `docs/contracts/local-control-api.md`,
  and two `storage.*` cases in the `service_code_cases` cross-check.
  - **The scan's own file list was the hand-maintained list `BRN-069` named, and it was hiding a
    family.** `production_codes_from_services` named three service files. `RepositoryError::code()` —
    six `storage.*` values that reach a client through every service's `Storage(_)` arm on a `500` —
    lives in a fourth, `repository/mod.rs`. Adding it made the completeness test fail immediately
    naming all six: `["storage.conflict", "storage.not_found", "storage.query_failed",
    "storage.row_corrupted", "storage.transition_refused", "storage.version_conflict"]`.
  - **The contradiction was already written down, in the negative.** The contract's own paragraph said
    the run-side conflict *"was reported as a `500` carrying `storage.version_conflict`, a code this
    table does not list"* — stating as fact that the table omitted a code the surface produced, in the
    document whose table claims to list every code the surface produces. `BRN-067`'s sentence about
    completeness was corrected in `BRN-071`; this paragraph is the same claim one paragraph over and
    was left standing. It is corrected rather than deleted, because the history it records is why the
    row now exists.
  - **The status is derived, and the derivation is why the delegation matters.** All six arrive on a
    `500` carrying the storage code rather than `internal.failure`, because a `Storage(_)` arm
    delegates `code()` and `retryable()` to the repository error. So `storage.query_failed` is the
    **only** retryable row — the repository answers "may this be sent again unchanged?" and a transport
    failure is the sole outcome a blind resend fixes — while a corrupted row is an answer, not a fault.
    Recording that required reading the repository's own `retryable()`, not guessing from the code name.
  - **Interception is per variant and per route, and it does not make the rows wrong.**
    `storage.version_conflict` is intercepted by all four services (as `resource.version_conflict` or
    `approval.version_conflict`) and `storage.not_found` by every service's `From`, so a client should
    never meet either on a `500` — which is a property of the routes that exists today, not of type
    safety, since `Storage(_)` is `pub` and one `map_err` bypasses every interception. Listing them is
    the conservative choice for codes a scan proves the surface can produce.
  - **The mapper cross-check now covers the storage family too**, so the six rows are not just present
    but asserted against the implementations: the two cases chosen are the ones where
    `RepositoryError::retryable()` answers differently, which is the distinction the delegation exists
    to preserve and the one a table-only check cannot see.
  - **Falsified:** removing the `storage.row_corrupted` row makes the completeness test fail naming
    exactly that code. Restoring it returns the gate to `0` (verified by `git diff --numstat`).
  - **Not done, and named.** The scan is still a list of four filenames rather than a discovery of
    every file that implements `code()` for a type reaching the envelope — so a fifth producer added
    later is invisible until named, which is the same shape `BRN-069` recorded one level up and
    `BRN-071` did not eliminate, only widened. `jarvis_domain`'s own `code()` values (`tool.*`,
    `approval.*` from `DurableApproval`) reach the surface through domain error types rather than
    through these four files and remain out of this scan's scope; they are covered by their own
    contracts' tables and by `BRN-065`'s namespace check.
  - 1582 workspace tests (unchanged). 193 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both
    docs gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-072` Make the scan's **file list** a checkable decision rather than a comment, and fix the
  truncation bug the new check exposed in the helper reading that list.
  Evidence: `the_envelope_producers_are_the_files_this_scan_reads` with its `SCANNED`/`EXCLUDED` lists,
  `strip_test_items` (brace-matching, moved to module level), the `walkdir` helper, and the corrected
  `production_codes_from_services` doc that now states the scope as a decision.
  - **The finding: `jarvis-application` defines `code()` in nine files and the scan read four.** The
    other five were excluded by reasoning that lived only in prose, so the next envelope producer would
    be invisible exactly as `repository/mod.rs` was between `BRN-067` and `BRN-071`. The reasoning turns
    out to be **correct** — `run_controller.rs` and `model.rs` carry `run.*` and `model.provider_*`
    through `ControllerError::Provider(error) => error.code()` onto the **run resource's `error_code`
    field**, read on a `200`, not into an envelope; and `context_assembly.rs`, `recovery.rs`, and
    `tool_recovery.rs` have no HTTP production reference at all. Being correct is not the same as being
    checkable, so it is now a test: a `code()` in a file that is neither scanned nor excluded fails,
    naming the file and the decision it needs.
  - **The check exposed a real bug in the helper it was checking.** `context_assembly.rs` declares its
    test module **mid-file** (`#[cfg(test)] #[path = "..."] mod tests;` at line 51, with the type under
    test below it), so a helper that truncates at the first `#[cfg(test)]` hides every `fn code(` after
    that point — which is why the walk first reported 8 files rather than 9. That is the identical
    defect `BRN-066` found in the Node finder's `productionHalf`, reproduced in Rust four rounds later,
    and it fails in the **quiet** direction: the cut removes definitions rather than adding them, so the
    file is simply absent rather than wrong. `strip_test_items` now brace-matches each item — a
    declaration ends at its semicolon, an inline block at its matching brace — and is used by both the
    scan and the walk.
  - **The four scanned files were never affected, and that is luck rather than design.** Their test
    modules are trailing (`#[cfg(test)]` within three lines of EOF), so truncation happened to be
    correct for them. A helper that is only right for the inputs it currently receives is a landmine for
    the next caller, which is why the fix is one shared function rather than two truncations.
  - **Both lists are checked in both directions.** Every `code()` file must be scanned or excluded
    (the omission case), no file may be both (the stale case), and every excluded name must still define
    `code()` (the renamed-or-moved case). Falsified by renaming the `context_assembly.rs` entry: the test
    fails naming that file and stating that the choice is the point.
  - **Not done, and named.** The `EXCLUDED` list is itself hand-maintained — a file that stops defining
    `code()` fails the third assertion, but a *new* file that defines `code()` and reaches the envelope
    still needs a human to move it from `EXCLUDED` to `SCANNED`, because no test can decide reachability
    mechanically. That judgement is now one line in one place rather than a sentence in a doc comment,
    which is the improvement this round could make; eliminating it would need type-level evidence that
    the code reaches `ErrorEnvelope::new`, which the workspace does not have.
  - 1583 workspace tests (+1). 194 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both docs
    gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-073` Document the **run resource's `error_code` vocabulary** — the codes that are a field on
  a `200` rather than an error envelope — and make the list checkable against its four producers.
  Found by continuing `BRN-072`'s trail: it closed the envelope producers, and the codes those rounds
  excluded turned out to have no enumerated home at all.
  Evidence: the run error-code table in `docs/contracts/local-control-api.md` and
  `the_run_error_code_vocabulary_is_the_one_the_producers_emit` in `jarvis-infrastructure`'s `http` tests.
  - **The finding: a whole client-visible vocabulary with no list.** `error_code` on a run resource can
    carry about twenty-five values, and two completeness tests had been built around the envelope set
    without either noticing that the *other* set was undefined. Nine were in **no document at all** —
    `run.stream_interrupted`, `run.output_not_persisted`, `run.clock_unavailable`,
    `run.context_budget_unusable`, `run.context_message_unlabelled`, `run.context_item_too_large`,
    `model.provider_unavailable`, `model.provider_timeout`, `model.provider_malformed` — and the rest
    were mentioned only where the code producing them happened to be discussed, which is not a list. A
    client reading `run.stream_interrupted` had nowhere to look.
  - **Why neither existing check could see it.** Both read this contract for a table whose rows carry a
    status, and these values never have one — they are not an envelope for a non-2xx response. So the
    codes a client branches on most often (a run that failed) were the least documented, and the
    omission was structural rather than an oversight: no test was even looking at the right shape.
  - **The four producers are named in the guard, and the codes are read from their `code()` bodies.**
    `ControllerError` (9), `ProviderError` (9), `BudgetLimit` (2), `AssemblyError` (3), and
    `RecoveryAction` (1) — the last four reached through the first. Reading each body rather than
    transcribing its arms means a new arm fails the test, which is the fix `BRN-067` and `BRN-071`
    applied to the envelope set, now applied where they could not see.
  - **The comparison is deliberately one-directional, and the reason is stated in the test.** Every
    emitted code must be documented; the reverse is not asserted, because event types share the `run.*`
    prefix — `run.context_building` is an event, not a code — and the two cannot be told apart
    mechanically. Requiring the listed set to be a subset of the emitted one would fail on a correct
    document, and a test that fails when the document is right is worse than no test. The direction that
    matters is kept: a value a client can meet is never undocumented.
  - **Falsified:** removing the `run.clock_unavailable` row makes the guard fail naming exactly that
    code. Restoring it returns the gate to `0`.
  - **Not done, and named.** The event types that share the `run.*` prefix are still not enumerated
    anywhere, so a client cannot look up `run.context_building` either — a smaller gap than this round
    closed and a different one, since an unrecognized event is skipped rather than misread, whereas an
    unrecognized failure code is a cause a client must act on. The one-directional comparison also means
    a **stale** row (documenting a code no producer emits) is not caught here; `BRN-065`'s namespace
    check would reject such a code if it ever reached the envelope, but nothing checks the field's list
    for rows nothing can produce.
  - 1584 workspace tests (+1). 195 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both docs
    gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-074` Close the residual `BRN-073` named and **correct that round's own "impossible" claim** —
  the run error-code table is now checked in **both** directions, and the reverse one found a real miss
  in the forward scan.
  Evidence: `run_error_code_set` and its `RunCodeProducers` struct in `jarvis-infrastructure`'s `http`
  tests, the bidirectional comparison in `the_run_error_code_vocabulary_is_the_one_the_producers_emit`,
  and the `NOT_THE_FIELDS` exclusion list with its staleness check.
  - **`BRN-073` said the reverse comparison was impossible; that was wrong in the narrowing direction.**
    It reasoned that event types share the `run.*` prefix — `run.context_building` is an event, not a
    code — and concluded the listed set could not be checked against the emitted one. That is true of a
    comparison against *every* `run.*` literal in arbitrary source, and false of a comparison against the
    table's **own** list, which is authored: it contains only codes, so a row nothing produces is a stale
    row. The earlier note generalised "cannot tell codes from event types in source" into "cannot check
    the reverse at all", and the distinction is worth keeping because it is the difference between an
    unbuildable check and a buildable one.
  - **The reverse check found a miss in the forward scan immediately.** `run.context_objective_dropped`
    is produced at a **call site** — `Step::failed(.., "run.context_objective_dropped")` — not returned
    from any `code()`, and `BRN-073`'s scan read a fixed window from each `code()` marker. So the table
    was right and the scan was wrong, and the only reason it surfaced is that the new direction asked a
    question the old one could not: "does every documented code have a producer?" reported the missing
    value as *unproduced*. A windowed scan looked precise and was not; the scan now reads each file's
    whole production half, stripping test items by brace-matching.
  - **A whole-file scan needs two subtractions, and one of them is a single value.** Event names live in
    the same files as codes, so they are subtracted — and they are read from `jarvis-protocol`'s own
    `event_type` constants rather than listed. Budget-construction codes (`run.budget_malformed` and the
    two range errors) are `run.*` values that never reach `error_code`, so they are excluded by a
    `NOT_THE_FIELDS` list whose every entry is checked to still be produced.
  - **`run.cancelled` is both an event type and a code, and getting that wrong broke the check twice in
    opposite directions.** Subtracting event types for both comparisons dropped it from the reverse set
    and made a *correct* table report `run.cancelled` as stale; not subtracting them at all made the
    forward check read event names as undocumented codes. Hence two sets: `every_value` answers "can a
    producer write this", `field_codes` answers "is this a code rather than an event name", and each
    comparison uses the one it needs. This is the same shape as `BRN-072`'s `SCANNED`/`EXCLUDED` one
    level down — a value that is two things at once needs to be accounted for as both.
  - **Falsified in two directions, each naming its own failure.** Inserting a stale row
    (`run.a_stale_row`) fails `a row for a value nothing produces sends a client looking for a cause that
    cannot occur`; renaming an entry in `NOT_THE_FIELDS` fails `these codes are excluded from the
    field's vocabulary but are no longer produced, so the exclusion is stale`. Restoring each returns the
    gate to `0`.
  - **Not done, and named.** The event types themselves are still not enumerated as a contract list, so a
    client cannot look up `run.context_building` — the smaller half of `BRN-073`'s residual, unchanged
    here because an unrecognized event is skipped while an unrecognized failure code is a cause a client
    must act on. The `NOT_THE_FIELDS` list is hand-maintained like every other exclusion in this
    workspace, with the same mitigation: it cannot go stale silently.
  - 1584 workspace tests (unchanged). 196 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both
    docs gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-075` Document a **third route a code can reach a client** that three separate scans each
  assumed did not exist — the `run.budget_*` codes, which arrive in a `400` envelope through a value
  *passed into* a mapper rather than written in one.
  Evidence: three `400` rows in `docs/contracts/local-control-api.md`'s minimum-code table, two new cases
  in `the_service_codes_status_and_retryable_column_are_what_the_mappers_send`, and the corrected
  `NOT_THE_FIELDS` reason in `run_error_code_set`.
  - **The finding: a code passed *through* a mapper is invisible to every scan of the mappers.** `budget_for`
    calls `RunServiceError::invalid(error.code(), ..)` — the code is an *argument*, so it appears as a
    literal in `run_service.rs` (which the envelope scan reads) but not in any `code()` body (which is
    what the four-file scan looks for), and `RunServiceError::Invalid` reaches the surface as a **`400`**
    through a mapper whose arms name no such code. It was invisible to the surface literal scan, to the
    envelope-producer scan, and to the run-field list — each for a different structural reason, and each
    for the same reason in spirit: all three look for *authored* code literals at a known place.
  - **A true exclusion reason was read as a stronger claim than it made, including by me.** `BRN-074`
    excluded these three with "a BudgetError from run construction; never stored as a run outcome" — which
    is correct, and I then reasoned from it as though it meant "never client-visible". They are visible,
    on a `400`. The exclusion from the *field's* list is right; the inference drawn from it was not, and
    the reason column now says where they do go.
  - **The guard is the mapper cross-check, not a scan**, which is why the fix had to include cases rather
    than only rows. `the_service_codes_status_and_retryable_column_are_what_the_mappers_send` drives the
    real `service_error_response` and asserts the status and both table columns; adding the two
    `RunServiceError::invalid` cases is what makes the three rows checkable at all. A scan over source
    cannot see a value that is only ever passed as an argument, so *driving the mapper* is the only
    instrument that reaches this class.
  - **Two of my three readings this round were wrong, both through too-narrow searches.** I reported
    `expiring_after` as having no callers (my pattern was `\.expiring_after\(`, missing the `::` path
    form — it is called in production at `run_service.rs:1387`) and the `DEFAULT_RUN_DEADLINE_MS ≤
    MAX_STEP_TIMEOUT_MS` ordering as unasserted (`the_defaults_are_bounded_and_finite` asserts it). The
    third reading — that the exclusion reason overstated its claim — was right. Recorded because the
    failure mode is the same one the memory notes describe for scanners: **a pattern that misses a
    syntactic form reports absence, and absence reads as a clean result.**
  - **The invariant the near-miss exposed is real even though the assertion already existed.**
    `budget_for` relies on `DEFAULT_RUN_DEADLINE_MS` being inside `MAX_STEP_TIMEOUT_MS` for every run a
    client creates; if the ordering inverted, every `POST /runs` would fail with a `400` carrying
    `run.budget_step_timeout_out_of_range`. The test that asserts it predates this round, so nothing was
    broken — but it is the reason these three codes need rows rather than a note.
  - **Falsified:** removing the `run.budget_malformed` row makes the mapper check fail with
    `the table must list run.budget_malformed as 400, because that is what the mapper sends: None`.
    Restoring it returns the gate to `0`.
  - **Not done, and named.** The class this round found is not closed: a code passed *into* any mapper
    from any source is still only covered where a case was written by hand, and nothing enumerates those
    pass-through sites. A mechanical check would need to know which `error.code()` calls feed a
    client-visible field or envelope — the same type-level evidence `BRN-072` recorded as unavailable.
    The two `run.budget_context_tokens_out_of_range` and `run.budget_step_timeout_out_of_range` rows are
    likewise covered by cases written for them rather than by a scan.
  - 1584 workspace tests (unchanged). 197 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both
    docs gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-076` Make the run-field vocabulary check see the **third** way a code reaches it — passed
  *through* a mapper from a domain type — and document the two codes that had no entry anywhere.
  Evidence: two rows in the run error-code table in `docs/contracts/local-control-api.md`, the
  `passed_through_codes` helper with its staleness assertion, and the widened `run_error_code_set`.
  - **The finding: `context_assembly.rs` maps a `DomainError` code into `AssemblyError::Refused { code }`,
    and that value reaches the run's `error_code` — with its only literal one crate away.**
    `ContextBudget::new` and `ContextBudget::assemble` return `ContextBudgetInvalid` and
    `ContextCandidatesUnbounded`; `context_assembly.rs` forwards `error.code()`; `ControllerError::
    ContextUnassembled { code }` carries it to the field. So `jarvis.context_budget_invalid` was in **no
    document at all**, and `jarvis.context_candidates_unbounded` was documented only as an **envelope**
    code (`400`/`403`) — a client reading it off a `200` run resource had no table for it.
  - **A whole-file scan of `jarvis-domain/src/error.rs` is the wrong fix, and the failure said so.** That
    file defines every domain `code()`, most of which never reach this field; reading it whole made the
    check reject **forty-one correctly-absent rows**. The right instrument for a passed-through value is an
    explicit list whose entries are verified against the defining type — the shape `NOT_THE_FIELDS` and
    `EXCLUDED` already use — because the question "which domain codes reach this field" is a fact about the
    call graph, not a property of any file.
  - **Two sets, used for different questions, and the pass-through codes join only one.** They enter
    `every_value` ("can a producer write this value" — a forwarded domain type does) and stay out of
    `field_codes` ("is this a code this vocabulary authors"), so the forward direction cannot be satisfied
    by a value no file here spells. That separation is `BRN-074`'s two-set design doing more work than it
    was written for.
  - **Falsified:** with the two codes removed from `passed_through_codes`, the reverse direction fails
    naming both as *causes that cannot occur* while the table documents them — which is what led to the
    helper rather than to a wider file list. Restoring returns the gate to `0`.
  - **A negative result this round, recorded because it is the majority of the work.** `BRN-075`'s
    residual — "codes passed into mappers are covered only where a case was written by hand" — sent me to
    enumerate every pass-through site. Three of the four are **unreachable today**, each for a different
    reason: `PolicyServiceError::Invalid { code }` from `PolicyException::grant` has no HTTP caller
    (`grant_exception` is invoked only from tests, and the policy routes call `active` and `evaluate`), the
    `ContextBudgetInvalid`-via-`ContextUnassembled` path carries const-literal reasons that always satisfy
    `TransitionReason`, and `ControllerError::ContextUnassembled`'s assembly-refusal source cannot fail in
    a way a client can trigger. Only the domain-code forwarding above was live. Saying so is the result:
    the class is narrowed, not closed, and the three unreachable sites would each become live with one
    route.
  - **Two of my own readings were wrong and were caught before they became entries.** I reported
    `ContextUnassembled` as unconstructed (it is constructed at three sites) and `model.exception_expired`
    as reaching a client through `grant_exception` before checking whether that method had a caller. Both
    came from acting on a grep result without confirming the reachability premise — the same failure the
    scanner notes describe, in a hand search.
  - 1584 workspace tests (unchanged). 198 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both
    docs gates), all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-077` Document the **event-type vocabulary** a client can receive — the residual `BRN-073`
  and `BRN-074` each named and left — finding two events the daemon publishes on every run that
  appeared in no document.
  Evidence: the event-type table in `docs/contracts/local-control-api.md`,
  `the_event_types_a_client_can_receive_are_all_documented` with its `declared_event_types` and
  `documented_event_types` readers in `jarvis-infrastructure`'s `http` tests, and the widened
  `the_event_type_names_the_stream_contract_requires_are_all_defined` in `jarvis-protocol`.
  - **The finding: two of the eleven event types were in no file.** The contract enumerated a
    "minimum event types" set of eight; the protocol's `event_type` module defines **eleven**. The
    two unlisted — `run.planning` and `run.responding` — are published by the controller on **every**
    run (`each_state_change_published_exactly_one_event` pins the seven-event sequence they appear
    in), so a client following the stream received two events whose names it could look up nowhere.
    This is structurally the same defect `BRN-073` found for the run resource's `error_code` field —
    a client-visible vocabulary with no list — one vocabulary over, and it survived two rounds that
    named it because each closed the *code* set and left the *event* set as residual.
  - **A guard that checks a floor cannot see a value above it.** The existing protocol test asserted
    the contract *contains* a required array of eight. So an event type the build sends that the
    document never names passed silently — the guard was scoped to "are the required ones present",
    which is true of a document that omits anything else. The fix reads the set from the protocol's
    own `event_type` module and asserts **derived equivalence** with the table, so adding a constant
    without documenting it now fails rather than shipping; the enumerated subset was replaced by the
    full set for the same reason.
  - **The reverse direction is checkable here, for the reason `BRN-074` established.** The table is
    authored and contains only event types, so a row naming a value no constant declares is a stale
    row — an event a client would wait for that the daemon never sends. Both directions are asserted.
  - **⚠ A collision the change had to handle, and it is the `run.*`-prefix problem again.** The new
    event table's rows are ``| `dotted.name` | … |``, which is shape-identical to the run error-code
    table's rows (`| `run.no_model_served` | … |`). The run-error-code completeness check parsed rows
    *by shape*, so once the event table existed it would have absorbed event types into the code set
    and reported a correct document as wrong. Both tables are now parsed **by their header** — the
    one thing that distinguishes two tables whose rows cannot be told apart, which is exactly the
    mechanism `BRN-074` could not use for the *source* (an event literal and a code literal share the
    `run.*` prefix with no header to separate them) but which a *document* provides.
  - **Falsified in both directions, each naming its own failure.** Removing the `run.responding` row
    fails with `every event type the build can send must be in the contract's event table … :
    ["run.responding"]`; inserting a `run.ghost_event` row that no constant declares fails with
    `… a row for an event nothing sends leaves a client waiting for one that never arrives :
    ["run.ghost_event"]`. Restoring both returns the gate to `0`.
  - **Not done, and named.** The set is checked against the protocol's constants, not against the
    controller's *publication* sites. The equivalence holds today because `advance` and
    `publish_usage` publish only through the constants, but a future path publishing a literal would
    not be seen — the same limitation `BRN-075` recorded for pass-through codes, since a value that
    never appears as a named constant is invisible to a scan of the constants. `stream.overrun` is
    declared alongside the run event types even though it names the *connection* rather than the run,
    which the module's own doc states; it is in the table with that note rather than excluded,
    because a client must handle it.
  - 1585 workspace tests (+1: infra 692). 199 TODO IDs (+1). All gates green (fmt, clippy, test, doc,
    both docs gates), 23 validator tests, all four journeys pass. **DO NOT COMMIT.**
- [x] `BRN-078` Serve the approval listing's **`risk` filter** — the residual `TLS-013` named and the
  one filter whose meaning changed when the step-up rule became real.
  Evidence: `ApprovalListFilter` (the port's filter value), the `?risk=` parse in the listing handler, the
  pass-through and sweep re-read in `ApprovalService::list`/`expire_lapsed`, and four tests —
  `a_risk_narrow_is_applied_before_the_page_bound_and_yields_the_served_position` (adapter),
  `a_risk_narrow_reaches_the_store_rather_than_being_dropped_on_the_way` and
  `a_lapsed_row_is_swept_without_widening_the_callers_risk_narrow` (service), and
  `the_listing_narrows_to_a_risk_level_and_refuses_an_unknown_one` (wire).
  *(The four `PENDING_RISK_*`/`PENDING_AFTER_*` SQL constants and the `approval_select!` macro this entry
  originally named were replaced by a `QueryBuilder`-built statement in `BRN-081`; the filter behaviour
  and the tests are unchanged, and only the adapter's assembly shape differs.)*
  - **The filter must run inside the `WHERE` the `LIMIT` bounds, and that is the whole design.** Applying
    it over an already-bounded page returns fewer rows than the caller asked for on a query that has
    more — a short page, which a client reads as "nothing more to decide". That is the exact defect the
    *channel* predicate was introduced to prevent, arriving by a second route, so the risk narrow reaches
    the statement rather than the handler and the sweep's re-read carries it too (a sweep that re-read
    without the filter would replace the caller's narrowed page with the unfiltered one).
  - **An unrecognised *value* of a known key is refused like an unknown key.** `?risk=severe` mapped to
    "no narrow" would return every risk level — a **superset**, the very thing the unknown-key refusal
    exists to prevent — so `Risk::parse`'s refusal is a `400 request.invalid`, asserted on the wire.
  - **⚠ The comment over the SQL constants was wrong, and consolidating them proved it.** It said the
    column list "the three statements below therefore repeat"; **four** statements did. `sqlx::query`
    cannot take a run-time-assembled string (it cannot audit one for injection, correctly), so the list
    is now interpolated by an `approval_select!` macro that expands to a single `concat!` literal — a
    reviewer reads one list, and a new statement cannot silently disagree about a column. The four
    listing shapes (narrowed? resuming?) exist as four constants because SQLite cannot index a nullable
    parameter, so a `(? IS NULL OR risk = ?)` predicate would degrade every first-page fetch to a scan —
    the same reason the cursor was already two statements rather than one. *(The four constants became one
    `QueryBuilder`-built statement in `BRN-081`; the nullable-parameter reason is why each predicate is
    pushed only when it applies rather than bound as a nullable value.)*
  - **The filter is a value, not a fifth positional argument.** `pending_in` already takes four values
    of three types with two adjacent integers; adding the narrow positionally invites a transposition
    the next scalar filter would make a compile error only by luck, and a second filter would then
    change every call site. `ApprovalListFilter` groups them, the same choice `RunRef`/`Step` record.
  - **Falsified four ways, each naming its own failure.** Applying the narrow with a wrong level
    (`map(|_| "high")`) fails the adapter test with `left: 0, right: 2`; dropping the filter at the
    service call (`ApprovalListFilter::default()`) fails `a_risk_narrow_reaches_the_store…` with
    `left: 2, right: 1`; dropping it on the **sweep's** re-read instead fails
    `a_lapsed_row_is_swept…` with `left: 2, right: 1` and the high-risk row in the body — the distinct
    detector the sweep needed, since no non-lapsing fixture reaches that re-read; and removing the
    parse's `Risk::parse` refusal would let `?risk=severe` through (the wire test asserts the `400`).
    Each restored to `0`.
  - **Not done, and named.** The other filters — `state`, `effect`, requesting run/tool, and the time
    filters — remain refused by name, each still needing its own indexed statement, by the same
    superset argument. The `risk` filter is a **single level**, not a set or a range: "critical or
    higher" would need an ordering predicate and a schema decision (the `Risk` ladder is ordered, so
    `>=` is available) that this increment did not take.
  - 1589 workspace tests (+4: infra 693, application 267). 200 TODO IDs (+1). All gates green (fmt,
    clippy, test, doc, both docs gates), all four hermetic journeys pass. **DO NOT COMMIT.**
- [x] `BRN-079` Wire the approval listing's **`--risk` and `--cursor` into the reference CLI**, so the
  cursor the daemon prints is a value an operator can spend rather than only read.
  Evidence: `ApprovalsAction::List`'s two new options and the `RiskArg`/`list_path` pair in
  `jarvis-cli`, the `jarvis_protocol::approval::risk` vocabulary, `Risk::ALL` in `jarvis-domain`, and
  four tests — `the_listing_accepts_a_risk_narrow_and_a_cursor`,
  `an_unknown_risk_level_is_refused_before_a_request_is_built`, and
  `the_listing_path_carries_exactly_the_filters_the_client_was_given` (CLI),
  `the_wire_risk_vocabulary_is_the_domain_ladder_in_ladder_order` (infra), plus the composed-surface
  steps in `tests/e2e/approval-journey.mjs`.
  - **The finding: the daemon served `?risk=` and handed back a `next_cursor`, and the reference client
    could send neither.** `approvals list` was wired for `--limit` only, so a cursor the CLI printed was
    a value an operator **could read and not use** — the exact dead end the cursor was introduced to
    close on the daemon side (`BRN-077`/`BRN-078`), one layer out. `TLS-013` names the CLI as a served
    surface, so a feature the API has and the CLI cannot reach is the CLI half of the feature missing,
    not a separate one.
  - **The wire vocabulary moved to `jarvis_protocol::approval::risk` so the CLI does not hold a second
    spelling.** The CLI depends on `jarvis-protocol`, not `jarvis-domain`, and the contract's rule is
    that the wire value is the contract's own spelling — so the four level names live beside the
    `ApprovalView.risk` field they must agree with, and `RiskArg` (a **closed set** parsed on the client)
    iterates `LEVELS` rather than a local list. `--risk severe` is a usage error before any request is
    built, rather than a `400` from a round trip: a client that forwarded an unrecognised level would be
    trusting the server to validate its own command line.
  - **⚠ `Risk` had no `ALL`, so "every level" had no definition.** `Effect` gained one precisely for the
    exhaustive-parse guard, and `Risk` — the value an ordering comparison and now a wire filter both turn
    on — had only the four variants and a `parse` match. Adding `Risk::ALL` gives `every_risk_level_…` a
    list to iterate, and `the_wire_risk_vocabulary_is_the_domain_ladder_in_ladder_order` (in
    `jarvis-infrastructure`, the one crate that sees both) holds the wire list to the domain ladder
    **including its order**, because a future "this level or higher" filter would iterate it.
  - **`list_path` is a pure function so the query string is checkable without a daemon**, asserted byte
    for byte (`limit`, `risk`, `cursor` in a fixed order) — the same technique `ask_body` uses. The
    cursor is inserted **verbatim**: it is base64url-without-padding behind a `v1.` prefix, so it is
    already URL-safe, and percent-encoding it would be wrong in the other direction because the daemon
    decodes the literal value it minted.
  - **Falsified three ways.** Dropping the risk push in `list_path` fails the composed-surface step
    (`**--risk critical must exclude the high-risk approval**`); reordering `LEVELS` fails both the
    protocol spelling test and the infra ladder cross-check independently (`left: ["low","high",…]`); and
    `--risk severe` is asserted to fail parsing rather than reach the daemon. Each restored to `0`.
  - **Not done, and named.** `--risk` is a single level, not a set or `>=` range (the `Risk::ALL` order is
    now available to build one); the other listing filters stay API-refused by name; and the CLI still
    does not *follow* a `has_more` page automatically — an operator passes `--cursor` explicitly, which
    is the honest shape while the listing is an inspection surface rather than a sync.
  - 1595 workspace tests (+6: cli 42, protocol 49, domain 501, infra 695; plus 3 composed-surface CLI
    steps in the journey). 201 TODO IDs (+1). All gates green (fmt, clippy, test, doc, both docs gates),
    all four hermetic journeys pass. **DO NOT COMMIT.**
- [x] `BRN-080` Bind the **risk narrow into the approval cursor**, which I left unbound in `BRN-078` and
  made reachable from the CLI in `BRN-079` — a page boundary that silently **skipped** rows.
  Evidence: `ApprovalCursor.risk` with its doc, the `encode_cursor`/`decode_cursor` risk segment, the
  service's mismatch refusal, the adapter's and double's risk-from-cursor mirror, and three tests —
  `a_cursor_bound_to_another_risk_narrow_is_refused` (service),
  `a_cursor_carries_the_risk_narrow_and_resuming_inside_it_skips_nothing` (adapter), and
  `the_cursor_round_trips_the_risk_narrow_and_accepts_a_legacy_cursor` (wire).
  - **The finding, and it is a defect I introduced two rounds ago.** The cursor bound the **channel** but
    not the **risk**, so a position minted under `?risk=critical` (the last *critical* row's
    `(expires_at, id)`) replayed under `?risk=high` made the `>` comparison skip every high row expiring
    before it. Those are rows the caller asked for and would never see — the "a skipped approval is a
    prompt nobody decides" harm the keyset bound exists to prevent, arriving through the **filter**
    instead of through an offset. The risk filter (`BRN-078`) created the hole and the CLI wiring
    (`BRN-079`) made it reachable by a real operator; neither round's tests used a cursor *and* a narrow
    together, which is why every gate stayed green.
  - **The rule: every narrowing the query applies must be carried in the cursor, not just the first one.**
    The channel was bound because it was the only narrowing when the cursor was written; adding a second
    narrowing without binding it is the defect, and it is invisible because both the fresh-page path and
    the resume path work in isolation. A cursor is a position *inside a specific query*, so a query with
    two predicates needs both bound.
  - **A mismatch is refused (`request.invalid_cursor`), and an un-narrowed cursor is tolerated.** Refusing
    risk-vs-risk crossings matches the channel rule and keeps the binding honest. A cursor with **no**
    recorded narrow names an un-narrowed *superset* position, so no row a narrower view wanted can fall
    behind it — and there the caller's narrow is applied *over* the position rather than dropped, or a
    narrowed page would return rows above its own filter.
  - **The encoding is additive and the decoder accepts the legacy shape.** The risk is a fourth pipe
    segment; a three-segment cursor (what this build issued before) decodes as un-narrowed rather than as
    invalid, because such a page genuinely was — refusing it would break a client mid-page over a format
    change it cannot observe. A fifth segment is still refused (reading a prefix is how a parser accepts a
    value the writer never produced), and an unknown level is refused rather than silently widened.
  - **The adapter and the in-memory double take the risk from the cursor on a resume, matching how they
    already took the channel** — the same agreement `the_double_and_the_adapter_agree_on_a_cursor_for_another_channel`
    enforces, extended to the second narrowing. The service refuses a crossing, so the two values agree on
    every real request; taking the cursor's is what keeps the adapter correct if called directly.
  - **Falsified**: disabling the service's risk-mismatch guard lets the mismatched cursor return a
    **high** row under a **critical** request (`must be refused` fails with the row in the body), which is
    both the skip and the widening at once. Restored to `0`.
  - 1598 workspace tests (+3: application 268, infra 697). 202 TODO IDs (+1). All gates green (fmt,
    clippy, test, doc, both docs gates), all four hermetic journeys pass. **DO NOT COMMIT.**
  - **Not done, and named.** The cursor's bindings are still hand-assembled pipe segments rather than a
    versioned struct, so a *third* narrowing would repeat this exercise unless it is extracted first.
- [x] `BRN-081` Replace the approval listing's **four hand-written `SELECT` statements** with one statement
  built from the predicates the request has — the follow-up `BRN-080` named — so a new filter is one
  guarded `push` rather than a doubling of the statement count.
  Evidence: the `QueryBuilder`-built statement and the `approval_columns!` macro in
  `jarvis-infrastructure`'s approval adapter; `DECIDED_SQL`/`LOAD_SQL` share the one column list through
  it and the four `PENDING_*` constants are gone.
  - **The finding: the statement count multiplied with the optional filters.** `BRN-078` added the `risk`
    narrow and the cursor, making four constants for (narrowed? × resuming?) × the column list; the
    contract names five more optional filters (`state`, `effect`, requesting run/tool, time), so the next
    one would have made **eight** statements, each a place a predicate could be wired into the wrong shape
    and each with its own bind order to get right. Selecting among constants is a hand-written state space;
    the multiplication is the defect, not the four copies.
  - **`sqlx::QueryBuilder` builds the `WHERE` from the predicates that apply**, so a new filter is one
    `if let Some(x) { builder.push(" AND x = ").push_bind(..) }` — the parameters after it stay in the
    order they were pushed, and there is no combination to enumerate. `sqlx` is a direct dependency
    (`sqlx-core` is on the workspace graph already, so this is not a new dependency).
  - **Every user value is still BOUND, never interpolated.** `push_bind` emits a `?` and a parameter; the
    only text the statement interpolates is the column list and the predicate fragments, none of which
    come from the request. The guarantee the four constants gave is kept while the assembly became
    conditional, which is the point — a `QueryBuilder` used to splice a value would be worse than the
    constants, not better.
  - **The column list is still ONE definition.** It moved into an `approval_columns!` macro that `concat!`
    accepts as an argument, so `LOAD_SQL`, `DECIDED_SQL`, and the built statement share it — the "one list,
    not four copies" property `BRN-078` established, preserved across the rewrite. (`push` takes
    `impl Display`, so a `concat!(approval_columns!())` fragment could be pushed too; the initial fragment
    is a `concat!` literal so the whole statement is still as literal as it can be.)
  - **Behaviour is unchanged, and that is what the existing suite demonstrates.** Every adapter test —
    the four narrow/cursor combinations, the keyset resume, the short-page guard, the risk binding — passes
    with the same 1598-test count, and the composed-surface journey (real daemon, SQLite, cursor + risk +
    channel) is green. A rewrite of a hot query that changed no test *would* be suspicious; here the tests
    are the evidence that the state space collapsed without changing an answer.
  - 1598 workspace tests (unchanged — a refactor with no new behaviour). 203 TODO IDs (+1). All gates green
    (fmt, clippy, test, doc, both docs gates), all four hermetic journeys pass. **DO NOT COMMIT.**
  - **Not done, and named.** The `QueryBuilder` is confined to `pending_in`; `LOAD_SQL` and `DECIDED_SQL`
    are still plain constants because their parameter lists are fixed (the builder exists for the listing,
    whose predicates vary). The cursor's bindings remain hand-assembled pipe segments — extracting them
    into a versioned struct is the other half of the `BRN-080` residual and is untouched here.
- [~] `TLS-011` Define plugin manifest and process supervision contract. **The manifest is defined as
  types and validated, and the lifecycle state machine is implemented; process supervision is `TLS-015`
  and is not implemented.**
  Evidence: `jarvis_infrastructure::plugin` — `PluginManifest::parse` plus the field rules, the
  `PluginManifestError` code set, and 16 tests including the contract's own example read from the
  document (contract test 1: "schema, compatibility range, platform, and unknown-field validation").
  - **The finding: the contract was accepted with no value behind it, so nothing refused a manifest whose
    executable escaped its root.** `docs/contracts/plugin-manifest.md` is `ACCEPTED` and lists ten
    required contract tests, and the module tree had no `plugin` — so contract test 1 had no subject and
    the entrypoint rule ("executable paths are package-relative, canonicalized, and cannot escape the
    verified installation root") was a sentence with nothing enforcing it. This is the definition half of
    the slice, and it is the half `TLS-014`/`TLS-015` must build on rather than re-derive.
  - **Every field rule is one whose failure is a filesystem, execution, or log-integrity fault.** The
    path rule is the one that matters most, and each clause defends a distinct way two strings name one
    file or one string leaves the package: an absolute path or leading separator, a `..` traversal
    (refused, not resolved — resolving discards the fact that the manifest asked to leave), an empty or
    `.` segment, a Windows drive/ADS colon, a **reserved device name matched on the stem** (so `con.txt`
    is refused and `console.log` is not), and a trailing dot or space (which Windows strips). The
    reserved-name check being stem-based is the half a whole-name or extension-ignoring check gets wrong.
  - **`id` and `publisher` are slugs; `name` is display text.** The contract's "display name or
    executable filename never identifies a plugin" is what makes `id` the field a grant is compared
    against, so it is held to a bounded lowercase slug rather than free text; `name` is bounded and
    control-free but otherwise unrestricted, because it is not an identity. A control character is
    refused in the name because it would corrupt a terminal or a log line.
  - **The digest requires its `sha256:` prefix.** A bare digest could be any algorithm, and comparing one
    against a `SHA-256` is the same defect `SchemaFingerprint` refuses — the prefix is what makes two
    digests comparable.
  - **⚠ The contract's own example carries a documented placeholder (`"sha256:..."`).** The golden-fixture
    test reads the example from the document, so it must substitute exactly that one value — and asserts
    the placeholder is still present, so if the contract changes, the fixture fails rather than silently
    parsing a different document. `$schema` is **modelled** rather than refused as an unknown field
    (`deny_unknown_fields` would otherwise reject the contract's own example) and is **not dereferenced**:
    honouring a URL from an untrusted document is how a manifest chooses its own validator.
  - **Falsified:** disabling the trailing-dot rule fails `an_executable_that_escapes_the_package_is_refused`
    with `bin/rt. must be refused: a trailing dot Windows strips`. ⚠ **A first mutation that disabled the
    explicit `..` check did NOT fail the test**, because `..` also ends with `.` and the trailing-dot rule
    caught it — two rules overlap on that input (defence in depth), so the `..` branch is now written out
    with a comment saying it is *stated* rather than inferred from a character class that happens to cover
    it. **A mutation that survives because another rule covers the same input is not a missing guard.**
  - **Not done, and named** (in the contract's new Implementation Status section): package provenance and
    signature verification, and the archive traversal/symlink rules (`TLS-014`, contract test 3);
    process supervision — timeouts, bounded capture, crash-loop quarantine, credential revocation
    (`TLS-015`); and installation and grants (both slices). The *lifecycle state machine* is now
    implemented below; what is missing is every producer that would move a plugin between its states.
    `parse` grants nothing: it produces a validated document, which is exactly what the contract says a
    parsed manifest is.
  - 1614 workspace tests (+16: infra 713). 205 TODO IDs (+1). All gates green (fmt, clippy, test, doc,
    both docs gates). **DO NOT COMMIT.**
  - **The plugin lifecycle state machine is now implemented** — the nine contract states and the
    operations between them as a domain table, which `TLS-015`'s supervisor consults rather than
    re-derives. Evidence: `jarvis_domain::plugin::PluginState` (`can_transition_to`, `allowed_targets`,
    `parse`, and the classification predicates) and six tests, including the exhaustive edge set and the
    two absent edges the contract turns on.
  - **The two rules that belong to the table, not to a caller.** The contract states "install without
    enabling" and "quarantine … never silently re-enables on package update"; both are encoded as
    **absent** edges rather than checks, for the reason `ApprovalState` records: a guard written as an
    early return is a branch that can be silently deleted, while an edge that does not exist cannot be
    walked. `Verified -> Enabled` simply is not a target, so an installer that enabled on install would
    be a code change against the table; `Quarantined -> Enabled` is absent too, which is **stronger** than
    the contract asks — no transition reaches `Enabled` from `Quarantined`, so a supervisor that forgot to
    compare the package version still cannot re-enable a quarantined plugin. The only exit is `Disabled`.
  - **A running plugin cannot be removed directly.** `Enabled`/`Unhealthy` have no edge to `Removing`;
    they must be disabled or quarantined first, so the child process is stopped before the package it runs
    against is taken away — the mechanical form of the contract's "stop child processes on
    disable/removal". The test asserts both halves (a running plugin may not be removed, a stopped one
    may), so it is not vacuous.
  - **The edges are asserted as a written-out set, not sampled**, the same technique the `ApprovalState`
    and `RunState` tables use — a removed edge would silently forbid a legal operation and an added one
    would silently permit an operation the contract never named.
  - **⚠ `Enabled` and `Unhealthy` have the same target *set* but are not the same arm.** Clippy's
    `match_same_arms` fired on `InstalledDisabled`/`Disabled` (which genuinely share targets) and I merged
    those — but merging `Enabled`/`Unhealthy` would have made `Enabled -> Enabled` and
    `Unhealthy -> Unhealthy` legal, the self-transition the table refuses. The comment says why the two
    pairs are treated differently, because "clippy said these arms are identical" is true of one pair and
    would be a defect for the other.
  - **Falsified both rules.** Adding `Verified -> Enabled` fails the edge-set and totality tests; adding
    `Quarantined -> Enabled` fails with `quarantined -> enabled legality must be false` and the
    `allowed_targets` cross-check. Each restored to `0`.
  - 1620 workspace tests (+6: domain 507). 205 TODO IDs (unchanged — the item is `[~]`). All gates green
    (fmt, clippy, test, doc, both docs gates). **DO NOT COMMIT.**
  - **Not done, and named.** The machine is states and edges only: no persisted record, no supervisor, no
    health probe, no crash-loop counter, and no audit emission (`TLS-015`). Nothing moves a plugin between
    these states yet — the same "a complete domain capability with no producer" that `TLS-002`'s registry
    recorded, and `TLS-015` is its first consumer.
  - **The installed source identity is now typed, and an update is a classification.** The contract's
    "Stable Identity and Provenance" section names the tuple that an approval binds to and an update is
    measured against, and it had no value. `jarvis_domain::plugin::PluginSourceIdentity` is the seven
    facts as a tuple — `id`, publisher, package digest, signature identity, source, version, protocol —
    with the display name deliberately absent (the contract's own "display name … never identifies a
    plugin"), and `PluginVersion` is ordered **numerically** so `1.10.0 > 1.9.0` (a text comparison would
    get that backwards), with a pre-release sorting below its release per `SemVer`.
    `SourceContinuity::classify` is the update question as a **pure function**: same publisher **and** same
    id **and** a strictly newer version, else no continuity. Continuity is the *input* to the contract's
    grant-carry-forward rule, not the rule — `TLS-015` still decides whether to carry a grant forward, and
    can refuse even when continuity holds (an expanded capability set).
  - **⚠ A second copy of a rule, found by writing the domain value.** The manifest validation had its own
    `is_plugin_id` and `is_canonical_sha256`; the identity needs the same two rules, and a domain value
    cannot depend on an infrastructure helper — so the rules moved to the domain
    (`is_plugin_identifier`, `is_canonical_package_digest`) and the manifest now **consults** them. The
    duplicate helpers are **deleted**, not left beside the new ones: two spellings of one rule is the
    defect class this project keeps finding, and `is_canonical_package_digest` is now the domain's third
    carrier of the `sha256:` rule (`ActionDigest`, `SchemaFingerprint`), which must agree with the other
    two or a value written by one would be refused by another while looking identical.
  - **Falsified both halves of the classification.** Relaxing `<` to `<=` fails
    `continuity_requires_the_same_source_and_a_strictly_newer_version` ("the same version must not be
    continuity" — a re-install claiming continuity it does not have); dropping the publisher comparison
    fails `an_identity_is_a_tuple_so_a_different_publisher_is_a_different_plugin` ("a different publisher
    is never continuity" — the impersonation case). Each restored to `0`.
  - 1626 workspace tests (+6: domain 513). 205 TODO IDs (unchanged). All gates green (fmt, clippy, test,
    doc, both docs gates). **DO NOT COMMIT.**
  - **Not done, and named.** No grant store, so `SourceContinuity` has no consumer yet — the carry-forward
    *policy* (proving no capability/schema/effect expansion) is `TLS-014`/`TLS-015`'s. The signature
    identity is carried opaque and bounded; validating it against a signature *kind* is `TLS-014`'s, since
    kinds (sigstore, and any other) are that slice's external vocabulary.
  - **The durable grant is now typed, and the carry-forward rule has one definition whose direction is the
    crux.** The contract's "Permissions and Grants" section names what a grant binds and states the
    carry-forward rule ("only under an explicit policy that proves publisher continuity and no
    capability/schema/effect expansion"), and neither had a value.
    `jarvis_domain::plugin::PluginGrant` binds the **whole** `PluginSourceIdentity` (not `id`) plus the
    workspace and granting principal; `applies_at` requires all four conditions together and
    `names_identity_but_expired` keeps "your grant expired" distinct from "you have no grant", because
    both refuse. `PluginCapabilitySelector` is a validated selector whose `Deserialize` **routes through
    the constructor**, since a selector reaches a persisted grant and the `Scope`/`WorkspaceRelativePath`
    precedence applies.
  - **⚠ The carry-forward rule was first written backwards, and two tests caught it at once.** The first
    `is_superset_of` asked whether the *new* grant conferred at least everything the old one did — which
    carries a grant across exactly the update the rule exists to stop (the update that grants **more**).
    "No expansion" is a **subset** on every dimension: every capability the new grant confers must already
    be conferred, and each ceiling (`risk`, `sensitivity`) must not have risen. `grants_no_more_than` is
    the corrected single definition, and `carry_forward_to` composes it with workspace, principal, and
    `SourceContinuity`. The schema dimension of "no expansion" is carried by the whole-identity comparison
    already in `carry_forward_to`, so it is deliberately not re-derived in the expansion check — two
    derivations of one condition is how a caller comes to apply one and forget the other.
  - **Falsified four mutations, each in its intended test, each restored to `0`.**
    (A) dropping `grants_no_more_than` from `carry_forward_to` fails
    `carry_forward_requires_continuity_and_no_expansion` with "a grant must not carry forward across a
    capability expansion"; (B) dropping the identity equality from `applies_at` fails
    `a_grant_applies_only_to_the_exact_source_identity_workspace_and_principal` with "the same id from a
    different publisher" — the impersonation case; (C) making `grants_no_more_than` always refuse fails
    the **complement** test (`a_narrowing_update_carries_forward…`), which is the control that stops the
    expansion test from being satisfied by an implementation that refuses every carry-forward; and
    (D) making `SourceContinuity::classify` always continuity fails three tests including the
    carry-forward one ("a different publisher is not continuity"). The complement and the mutation-C
    pairing is the point: an assertion and its control, falsified separately.
  - 1631 workspace tests (+5: domain 518). 204 TODO IDs (unchanged — the item is `[~]`). All gates green
    (fmt, clippy, test, doc, both docs gates). **DO NOT COMMIT.**
  - **Not done, and named.** No grant store, so `PluginGrant` and `carry_forward_to` are the contract's
    vocabulary with **no producer and no consumer** — scaffolding, labelled as such in the contract's
    Implementation Status rather than implied to be wired. `TLS-015` still owns the decision to carry a
    grant forward (it may refuse even when all four conditions hold) and every producer that would write
    one.
- [~] `TLS-012` Prove native/MCP/runtime routes cannot bypass policy. **The native route is now
  proven and reachable; the MCP and runtime routes do not exist yet, so their proof is not
  attempted.** The native proof is not a test over a hypothetical path — it is the only path a
  native tool has, and the milestone's real defect was that the path did not exist at all.
  Evidence: `jarvis_application::tool_call` owns the single governed pipeline
  (`resolve -> validate -> fingerprint -> evaluate -> reserve -> execute -> record`), the
  infrastructure adapters in `jarvis_infrastructure::tool_adapters` and the first real native tool
  (`clock.now@1`, `native_tools::clock`) are its production callers, and the run controller now
  dispatches model-proposed tool calls through it instead of refusing every intent with
  `run.tools_not_implemented`. **By construction rather than by assertion**, which is the only
  version of this claim that holds: `ToolCallService::invoke` is one method, every step is a call
  inside it, and a second entry point would be a second pipeline — and the one a caller reached
  first would be the one that governed. The tests below are what make the construction checkable.
  - **`EXECUTING` is durable before the effect, and the assertion is made from *inside* the effect.**
    The contract's rule is about ordering, so a read taken after `invoke` returns a *later* state and
    would pass for an implementation that wrote `Executing` afterwards. The recording executor reads
    the ledger while its own body is running and returns it to the test, and the three writes
    (`Approved`, `Reserved`, `Executing`) are separate rather than one combined write for the same
    reason: a single write carrying state-and-outcome would place `EXECUTING` on disk only after the
    world changed, which is the window a crash duplicates an effect in. Falsified by moving the
    `Executing` transition after the `execute` call, which fails
    `executing_is_recorded_before_the_effect_runs` on the observed state alone.
  - **A refusal is an outcome, never an error.** `ToolCallOutcome::Refused` carries a
    `ToolErrorClass`, and a denied call, an unknown tool, and invalid arguments all arrive as it —
    because a model that is not told "you are not allowed to do that" proposes it again, and
    collapsing these into `Err` would make the controller report a *run fault* for a working
    refusal. The refusal detail is rendered into the observation the next turn receives, so the loop
    is closed rather than merely recorded.
  - **A consequential tool cannot be dispatched by a grant alone.** The `email.send@1` fixture is
    granted and *still* waits: policy's `Allow` requires a **positive** property (read-only **and**
    low risk), so a tool that declares `allow` while doing something consequential is asked about
    anyway. The load-bearing assertion is the executor's call count — zero — because a pipeline that
    ran the effect and *then* asked would make every approval decorative, and a test that asserted
    only the outcome would not distinguish the two.
  - **The grant source that ships is the reviewed one, and it is narrow on purpose.**
    `NativeReadOnlyGrants` confers a grant only for `SourceKind::Native` **and** `risk == Low`
    **and** effects exactly `{ReadOnly}`. The conjunction is the point: each condition alone is
    satisfied by a tool that should not have been granted (a native tool that writes, a low-risk
    tool that sends, a read that also deletes), so the predicate is asserted from the refusing side
    as well as the granting one.
  - **A repeat is answered from the ledger, and the count proves it.** Two identical calls produce
    one execution and a `Duplicate` outcome, which is the difference between "the ledger is read" and
    "the ledger is written and then ignored". The ambiguous arm is the complement:
    `OutcomeAmbiguous` is **never retried**, because repeating a call whose effect may or may not
    have happened is how one effect becomes two.
  - **⚠ One logical call used to have two identities, and only a cross-record assertion could see
    it.** `raise_approval` minted a **fresh** `ToolCallId` for the approval's `tool_call_id` instead
    of the ledger row's own, so the durable prompt a user decides named a call that appears in no
    ledger row — and nothing could connect the decision to the row it releases, which is the read a
    resume needs. Every assertion in the pipeline suite stayed green, because none of them compared
    the two records: the test that catches it re-reads the **ledger row** through
    `awaiting_conversion` and compares it with the stored prompt, so it cannot be satisfied by two
    values built from the same expression in two places. Falsified by restoring
    `ToolCallId::from_uuid(now_v7())` in the request, which fails
    `a_prompt_names_the_very_row_the_ledger_opened` with "the prompt must name the ledger row's own
    call identifier, not a second one". The identifier is now derived once
    (`canonical_call_id`) and carried into both records. The approval's raised arguments also stopped
    being re-derived from a second source: the call id travels **from the row**, so the pair cannot
    drift.
  - **The loop, end to end, against a real controller.** `run_controller_tests` drives a run whose
    model proposes a tool call and asserts the **exact 11-event sequence** rather than an outcome,
    which is what caught two real defects: a `ToolResult` published without its `ToolCall`
    (`jarvis.orphaned_tool_result`) and a transition attempted straight from `Observing`
    (`jarvis.run_transition_not_allowed` — the loop must go `Observing -> Planning ->
    AwaitingModel`). Both are invisible to a test that only checks the final answer.
  - **Not done, and named.** The MCP route (`TLS-008`/`TLS-009`) and the external-runtime route
    (`TLS-011`/`TLS-015`) have no transport yet, so no test can yet show *their* calls arriving at
    the same pipeline; when they exist, the proof they need is that they reach `invoke` rather than a
    parallel path, and this item stays `[~]` until that is asserted. The result body is not durable
    in the ledger — a `Duplicate` reports the recorded class and state, not the recorded output —
    and `ClockUnavailable` is a service error rather than a tool outcome, so a clockless daemon
    fails the call rather than refusing it. **And a waiting call cannot yet be resumed, for a reason
    that is now structural rather than merely absent:** an approved call must reserve and dispatch
    through this same pipeline, which needs the arguments it was proposed with, and the ledger
    stores none (`tool_call_records` has no arguments column — the row is about whether an effect may
    exist, not about how to perform one). Dispatching the call when the approval is decided would
    therefore need the arguments re-derivable from a durable proposal record, and without one the
    honest options are "nothing dispatches it" or "it is dispatched with a second copy of the
    arguments that could differ from the fingerprinted ones — the exact defect the fingerprint
    exists to prevent". So a granted prompt currently leaves a `waiting_approval` row whose key keeps
    its reservation: a later duplicate is refused `Conflict` rather than answered, and startup
    recovery ends the row `cancelled` as a safe-to-retry pre-dispatch call. That is fail-closed, and
    it is written down here rather than implied to work.
  - 1646 workspace tests (+1). 211 TODO IDs (unchanged — the item is `[~]`). All gates green
    (`fmt`, `clippy -D warnings`, `doc`, `test`, both docs gates). **DO NOT COMMIT.**
- [~] `TLS-013` Implement authenticated approval list, preview, decide, expire,
  revoke, and resume use cases for API and CLI with channel assurance checks. Owns
  `jarvis_application::approval_service`.
  Evidence: list, read, decide, cancel, and the expiry-on-read transition are implemented over the
  wire, and the composition root is proven by an end-to-end journey.
  - **The server-derived decider is structural, not remembered.** `DecideApprovalRequest` has no field
    for the principal, channel, assurance, or time, so there is nothing for a handler to forget not to
    read — the same argument `BRN-024` made for policy grants. The service takes a `RequestContext`,
    which only trusted code can build, and the journey asserts the recorded `decided_by` equals the
    principal the server resolves rather than one the caller named.
  - **A repeat is idempotent and is checked before the version.** The contract requires "same-key/
    same-request retry returns the original decision", and a same-request retry carries the
    `expected_version` from the original body — so a version check first would refuse the ordinary
    double-tap as stale and defeat the rule. The state is what the caller asked for, nothing is
    overwritten, and `applied: false` says this call did not perform it.
  - **Fingerprint before version.** A re-approval cannot fix a digest that still will not match, so
    reporting "stale version" would send the user to do exactly that.
  - **Expiry is evaluated on every read and the lapse is *recorded*.** A record that lapsed while
    nobody was looking still reads `pending` in storage, so a refusal that did not write the `expired`
    transition would leave a prompt nobody can decide in every later listing. **For several rounds only
    `decide` did this**, while the module's own doc rule 3 and the contract both said "every
    read/decision/reservation" — so the two surfaces that *show* a prompt reported a record the daemon
    then refuses. `list` and `read` now take the instant and sweep it. The listing **expires and
    re-reads** rather than filtering, because a filtered page comes back short and on a cursor-less
    surface a short page is how a client concludes there is nothing left to decide — the same defect
    the channel filter caused here once, by a different route. **Falsified twice**: with the listing
    sweep bypassed the service test reads `left: 2, right: 1` (the lapsed rows returned) and the journey
    fails `exactly one approval is live (the second is lapsed), got 2`; with the detail sweep bypassed
    it fails `left: Pending, right: Expired`. The journey seeds **two** rows, one already past its
    deadline, so the listing's count is itself the assertion, and asserts the stored state and audit
    row rather than only the response — hiding a lapsed row would satisfy the response check alone.
  - **A version conflict from the store maps to `approval.version_conflict`, not to a storage code.**
    `RepositoryError::VersionConflict` would otherwise reach a client as `storage.version_conflict`
    under a `500` — telling it the daemon faulted when its own view was merely stale, which is the one
    case where a retry after a re-read works. Round 57 fixed the identical conflict on the run surface.
  - **The listing filters by the caller's channel inside the query, and the page bound follows it.**
  - **`axum`'s `query` feature is deliberately not added**, so the listing's `limit` is parsed from the
    URI by hand and an unknown filter is refused by name rather than silently ignored — ignoring one
    would return a superset of what the caller asked for. Adding a dependency feature is a
    research-gate change, not a convenience.
  - **The listing's page bound applies to what the caller can decide.** `allowed_channels` is tested with
    `json_each` **inside the query**, before the `LIMIT` — an earlier version applied the bound to the
    whole workspace and filtered afterwards, so a page could be spent on rows the caller could not act
    on and come back short; with no cursor, a client that receives a short list concludes the queue is
    empty and does nothing. **Falsified twice**: a post-filtering service fails three tests with
    `left: 0, right: 1`, and dropping the SQL predicate fails four adapter tests. The store reads **one
    row more than the bound** so `bounded` is observed rather than inferred from `len() == limit`, the
    page order is `(expires_at, id)` so a tie cannot show one row twice and hide another, and the
    response carries `has_more`.
  - **The cursor is now served, and the doc that said otherwise was wrong about the reason.** `has_more`
    without a cursor was a dead end — the daemon told a client more prompts awaited a decision and gave it
    no way to fetch one, the worst of both because the client knows work remains and cannot do it. The
    previous note claimed a cursor "would claim a stable position the listing does not yet guarantee
    across a concurrent decision"; **the position was always stable**: the order is `(expires_at, id)`,
    `expires_at` is an immutable column and `id` is primary, so the order is total. What a concurrent
    decision changes is *membership*, not order — which is precisely why the cursor is a **keyset**
    position (`(expires_at, id) > (last.expires_at, last.id)`) rather than an offset. An offset *would*
    be the unstable choice: a row decided between two fetches shifts every later row by one, so page two
    skips an approval, and a skipped approval is a prompt nobody decides. `ApprovalCursor` carries the
    channel it was minted for and a cursor replayed against another channel is `request.invalid_cursor`
    rather than an empty page, because the empty page *is* the disclosure. The value is opaque (a
    versioned base64 payload) so a client passes it back rather than constructing one.
  - **Two SQL constants rather than one statement with a nullable resume parameter.** A single statement
    would need `(? IS NULL OR (expires_at, id) > (?, ?))`, which SQLite cannot index — silently degrading
    every first-page fetch to a scan — or a string built by concatenation, which is how a user-supplied
    value reaches a statement. The bound is a **row-value** comparison matching the `ORDER BY` exactly;
    comparing `expires_at` alone with `>` drops every row sharing the boundary instant, which is the tie
    the identifier exists to break. **Falsified by dropping `id` from the comparison**: the resume test
    then fails, because a row removed between pages shifts the window.
  - **The expiry re-read resumes from the caller's own position.** `expire_lapsed` sweeps a page and
    re-reads; re-reading page one would answer a different question than the caller asked — a request for
    page three would receive rows from the beginning, and the cursor it held would then be the wrong one,
    so its next fetch would jump backwards. The same `after` is passed to every pass.
  - **Journey check added** (`tests/e2e/approval-journey.mjs`): a third live row is seeded so a bound of
    one is genuinely bounded, `has_more: true` must come with a `next_cursor`, the cursor is then used and
    the next page must be a **different** row, an unusable cursor is `400 request.invalid_cursor`, and a
    complete page omits the cursor. The journey's pre-existing "exactly one approval is live" count was
    updated to two rather than weakened — it is the listing half of the read-path expiry assertion, and
    keeping it exact is what stops the new row from becoming a third live one unnoticed.
  - **The in-memory double and the adapter disagreed about where the resuming channel comes from.** The
    adapter binds `cursor.channel` on the resuming statement; the double used the `channel` parameter.
    Production never saw it, because `ApprovalService::list` refuses a cursor whose channel differs from
    the caller's — so every test that went through the service passed while the two stores enforced
    *different* rules, and the diverging input (a forged cursor) is exactly the one a caller can only
    produce by bypassing the service. Fixed the double and added
    `the_double_and_the_adapter_agree_on_a_cursor_for_another_channel` in **`jarvis-infrastructure`**, the
    only crate that depends on both — the arrangement `the_reported_page_bound_is_the_one_the_store_enforces`
    already uses. **The first falsification passed**, because the seeded row permitted *both* channels and
    so could not separate the two paths; the row now permits only `api` and the mutation fails with
    `left: 1, right: 0`.
  - **The contract's cursor wording names a binding this surface cannot have.**
    "opaque **workspace/principal**/query-bound" — but the listing is deliberately **not principal-scoped**
    (an operator view must show prompts waiting on somebody else), so two of the three named bindings
    cannot both hold. The workspace half needs no cursor field and is *stronger* for it (it comes from the
    authenticated context on every request, applied unconditionally); the channel half is carried because
    it is the caller's own choice rather than the authenticated identity. Said so in the contract rather
    than fabricating a field to satisfy the wording.
  - **Two of `000008`'s three indexes cover queries that do not exist.** The migration says each covers
    "a query the port actually has" and names `pending_in`, `decided_by` ("what did I approve?"), and a
    lookup "every tool call performs — is there an approval for this call?". The third has **neither a port
    method nor a caller**, and `decided_by`'s method exists but is called from nothing but its own adapter
    tests. Recorded in `docs/data/schema.md`: one index for the listing, one for a method with no caller,
    one for a query nobody wrote. The indexes are right for the queries they were designed around; what is
    missing is the queries.
  - **The page bound had two definitions and nothing held them together.** `MAX_PENDING_PAGE` (the
    store's clamp) and `MAX_APPROVAL_PAGE` (what the listing *reports* as `max_page`) were two literals
    holding `200` with no comparison anywhere — and neither owning crate can make one, because the
    documented flow is `Protocol --> Domain`, so `jarvis-protocol` may not depend on
    `jarvis-application` and application may not depend on the wire vocabulary. Raising one alone would
    have made the daemon advertise a page size larger than the one its own query applies, so a client
    paging by `max_page` would receive fewer rows than the response claimed and could not tell a full
    page from a truncated one. The comparison now lives where it is expressible —
    **`jarvis-infrastructure` depends on both** — as
    `the_reported_page_bound_is_the_one_the_store_enforces`, the same arrangement
    `MAX_RUN_INPUT_BYTES`/`MAX_OBJECTIVE_BYTES` uses. **Falsified**: setting `MAX_APPROVAL_PAGE` to
    `201` fails it with `left: 201, right: 200`.
  - `tests/e2e/approval-journey.mjs` (19 checks) seeds a pending approval directly — no executor exists
    to create one over the API — and proves the composed surface answers, a decision records the
    server-derived actor, a repeat reports `applied:false`, the audit trail has one row, a fingerprint
    mismatch changes nothing, **the CLI's `show`, `list`, and a refused decision reach the same
    daemon**, and the decision survives a restart. **Falsified: removing
    `.with_approvals(...)` from the daemon turns every request into `503 service.not_ready` and fails
    six checks while the whole handler suite stays green** — the composition-root defect `BRN-014`
    fixed for the policy surface, and the reason a composition needs an end-to-end test.
  - **The detail view now carries the tool's source and schema identity**, which the contract's detail
    requirement names and the wire did not. `tool_id` alone is the **capability**, and `ACC-024` is the
    rule behind the requirement: an approval binds to the *implementation*, so a client shown only
    `mail.send@1` could not tell that the tool behind it had been replaced — the review decision detail
    exists to support. `ApprovalView` gained `tool_source_kind`, `tool_source_owner`,
    `tool_source_version`, and `schema_fingerprint`, **four fields rather than one joined tuple**, so a
    reviewer sees which dimension changed. Asserted **by value through the router** for all five, and
    falsified by projecting the source owner from the capability — which fails with
    `left: "mail.send@1", right: "acme.mail"`.
  - **The schema fingerprint now has a computation, which its own doc had claimed for several rounds.**
    `SchemaFingerprint`'s doc said "`jarvis-infrastructure` provides the computation" and no such function
    existed — the only construction path was `from_bytes`, which every caller in the product and in the
    tests used with a hand-written seed. So a schema change could not move an identity, and the tool
    contract's "a release that alters the input schema changes the fingerprint and therefore the identity"
    plus `ACC-024` had nothing behind them. `tool_fingerprint::schema_fingerprint_of` is the derivation,
    domain-separated with a `tool-schema:` prefix (trailing NUL, which no JSON document can start with) so
    a schema fingerprint cannot collide with an action fingerprint over the same bytes. The document is
    hashed **as the bytes given rather than re-serialized**, the same rule the arguments follow. **Falsified**:
    removing the separator hashing fails `a_schema_fingerprint_cannot_collide_with_an_action_fingerprint`
    and `a_schema_fingerprint_is_not_the_bare_digest_of_the_document`.
  - **The decision's assurance is now recorded, which the contract's Audit section requires and the record
    did not carry.** It is the fact that distinguishes a decision by a stepped-up caller from one by an
    ordinary session — the channel says *where* a decision was made, the assurance says how strongly the
    caller was authenticated. Taken from the **request context**, never a body field (`BRN-024`'s rule), and
    added to `ApprovalActor::Decided` plus a `decided_assurance` column (`000010`). **The first
    implementation of the column was wrong in a way only the second assertion caught**: the value was
    written from each *transition's* actor, so a decision stored `elevated` lost it the moment the approval
    was consumed — the consumption's actor carries no assurance and the update wrote NULL over it. The field
    therefore lives on the **record** beside `decided_by` and `decided_via`, and the test asserts both halves
    (after the decision, and after a further non-decision transition). **Falsified three ways**: taking the
    value from the actor makes the record read back as corruption; defaulting an unrecognised spelling fails
    the closed-vocabulary test; and two reader tests pin `NULL` (not recorded) and an unknown string
    (uninterpretable) as **different** facts, neither of them `Standard`.
  - **The decision's `comment` and the cancellation's `reason` are now stored and readable.** Both wire
    types declared them with docs claiming they were stored — the cancellation's said the reason "is what
    makes the audit trail say why a prompt was withdrawn" — and **nothing stored either**: each was
    deserialized, bounded at one layer, and dropped. They now live on the transition the human authored
    (`ApprovalActor::Decided::note`, `ApprovalActor::Cancelled::reason`) as a `DecisionNote` whose bound and
    control-character rule are enforced by its constructor **and its deserializer**, and the port gained a
    `transitions` reader because a writer with no reader is the same defect in the other direction — the
    fifth time this project has found it. The note is returned by the **detail** route (a listing would be
    one trail read per row) and an absent note is omitted rather than rendered as an empty one. **Falsified**:
    stripping the note in the writer fails `the_trail_returns_each_step_with_the_note_the_human_left` with
    "the decision's note must be readable from the trail".
  - **Two bounds for one kind of value collapsed into one.** `MAX_CANCEL_REASON_BYTES` (512) and the new
    note bound were the same number written twice, so the service's constant is now an alias of
    `MAX_DECISION_NOTE_BYTES`: a decision's comment and a cancellation's reason are one kind of thing, and a
    second literal would let one be raised alone while the domain still refused the longer value — which
    reads to a caller as an unexplained `request.invalid`.
  - **Not implemented, named in the contract's own Implementation Status section:** the grant-revoke
    route (no standing-grant store), approval creation (no executor, so nothing calls `request`), the
    **durable expiry worker** (the contract names two evaluators and only per-read exists, so a row
    nobody reads stays `pending` with a passed deadline — it belongs with the scheduler `AUT-002`
    adds, as another instance of the reaper that slice owns), the remaining list **filters** (`state`,
    `risk`, `effect`, requesting run/tool, and time — the **cursor** is now served, see below),
    `CONSUMED`/`INVALIDATED` (they
    need a reservation through the ledger), the outbox/resume signal (`AUT-004`), preview redaction
    (the producer redacts; there is no producer), the detail view's remaining named
    inputs (`output_schema` fingerprint, artifact and content hashes, connector account/resource
    resolution — each needs a producer that does not exist), cross-language fingerprint vectors, and
    generated OpenAPI.
  - **The step-up rule now has a producer, and finding it exposed a variant that could never fire.**
    `approval.assurance_insufficient` is in the contract's stable-error list and **nothing produced it**:
    `ApprovalServiceError::InsufficientAssurance` was constructed only by a `_ =>` arm inside
    `required_assurance_of`, and that function's inner call returns exactly one variant
    (`Unauthenticated`) — so the arm was unreachable code that made the refusal *look* produced, the
    same dead-protection shape as the terminal-state guard in `ApprovalState::can_transition_to`. The
    assurance was resolved and *recorded* as an audit fact (`ApprovalActor::Decided.assurance`) while
    nothing ever *compared* it, so "critical actions default to step-up" was a contract sentence with
    no code behind it and an ordinary session could decide the one class of prompt the risk label
    exists to make a user step up for.
    - **The requirement is derived, not configured.** `RequiredAssurance::required_for(Risk)` owns the
      threshold — `Critical` requires `Elevated`, everything else `Standard` — so there is one answer
      rather than two values that must agree, which is why no `required_assurance` column is added.
      `High` deliberately does **not** step up: stepping up everything above `Moderate` would make the
      label meaningless, because `High` is what an ordinary write is classified as. The rule is
      asserted over the **whole** four-rung ladder with the expected table written out by hand, so a
      risk level added without an answer fails rather than silently inheriting one.
    - **Enforced in `decide`, after the channel check**, and the position is a decision: the two
      refusals send the user to different places — `approval.channel_not_allowed` means "use another
      surface", while `approval.assurance_insufficient` means "prove who you are again" on the surface
      they are already on. `a_channel_refusal_outranks_an_assurance_refusal` pins it, because a request
      that violates both must not report the remedy that cannot succeed.
    - The comparison is the ladder (`is_satisfied_by`), not equality, so a stepped-up caller is not
      refused a standard requirement — the one refusal that pushes toward weakening a requirement.
    - **Falsified six ways, zero survivors:** removing the check (killed by the service test),
      never satisfying the requirement (7 tests), requiring step-up for every risk, treating critical
      as ordinary, inverting the ladder (3 tests), and demoting the channel check below the assurance
      one. The **wire** test was falsified separately against the removed check, so it is not vacuous:
      it asserts the `403`, the contract's own code, `retryable:false`, and that the refused decision
      left the record `pending` at version 1 — a refusal that had already written the decision would be
      the worst direction.
    - **What is still missing, and named:** a per-record override, so an approval cannot require
      *more* than its risk implies. That needs a field plus a producer (policy) that does not exist,
      which is why the derived rule is the honest increment rather than a half-wired column.
  - **The list filters remain unimplemented, and they stay refused by name rather than ignored.**
    `state`, `risk`, `effect`, requesting run/tool, and the time filters are each a real query the port
    does not have. They are refused explicitly because an ignored filter returns a **superset** of what
    the caller asked for — a client believing it is looking at refused prompts while it is being shown
    every one — which is the same disclosure direction the channel predicate exists to prevent. Adding
    one is a port method, an adapter statement, and a wire field together; the `risk` filter in
    particular is now meaningful in a new way, since a client may want the critical prompts that will
    demand a step-up.
- [ ] `TLS-014` Implement plugin package provenance/signature verification,
  compatibility validation, install-disabled, staged update/rollback, disable,
  data-retention choice, and removal.
- [ ] `TLS-015` Implement plugin grants, scoped launch environment, process
  supervision, resource limits, health, crash-loop quarantine, and audit.
- [ ] `TLS-016` Define and implement the skill contract
  ([skill-contract.md](docs/contracts/skill-contract.md)): a persisted skill record
  with a content hash over every loadable file, trust tiers whose install policy
  differs by source, progressive index/body/reference loading recorded in the
  context manifest, scan-before-use, quarantine of a dangerous verdict, and a
  provenance lockfile. The authority-narrowing invariant — a skill may remove tools
  from a call's catalog and may never add a tool, raise a risk ceiling, relax an
  approval requirement, or change an effect classification — is implemented as a
  tested refusal against a skill that declares the expansion, not only against a
  conforming one. See [ADR-0012](docs/adr/0012-governed-learning-loop.md).

## Milestone 4: Memory

Dependencies: Milestone 3 exit gate.

- [ ] `MEM-001` Define typed memory and provenance schema.
- [ ] `MEM-002` Implement user-confirmed preference memory lifecycle.
- [ ] `MEM-003` Implement lexical retrieval and deterministic ranking baseline.
- [ ] `MEM-011` Implement the procedural memory lifecycle and the learned-skill
  record: a learned procedure is a durable memory carrying the provenance every
  memory class carries plus the canonical identities and schema fingerprints of the
  tools it references. Learning is deny-by-default and disabled until a
  workspace-scoped grant enables it, and a learned procedure is bound to the grant
  for its exact content hash. See
  [memory-context.md](docs/architecture/memory-context.md) and
  [ADR-0012](docs/adr/0012-governed-learning-loop.md).
- [ ] `MEM-012` Implement the skill authoring tools (`skill.author`, `skill.patch`,
  `skill.delete`) and `memory.promote` as canonical tool calls, each with declared
  effects, risk, timeout, and an idempotency key derived from the candidate so a
  retried write cannot duplicate it; and implement inspect-with-provenance,
  correct, revoke, export, and delete for learned skills and promoted memories.
  Assert the absence of any write path that skips the tool fabric.
- [ ] `MEM-013` Implement the candidate→commit promotion pipeline for learned
  procedures: extraction may use a model, deterministic policy decides whether a
  candidate may be written, a sensitive candidate requires confirmation, review may
  be staged with a diff that survives restart, and hidden chain-of-thought is never
  stored or derived from.
- [ ] `MEM-004` Research and implement embedding adapter with version metadata.
- [ ] `MEM-005` Implement hybrid retrieval and query-time workspace filtering.
- [ ] `MEM-006` Implement entity candidates, confidence, merge, and split history.
- [ ] `MEM-007` Implement inspect, correct, supersede, archive, forget, export, and
  memory-disable flows.
- [ ] `MEM-008` Implement context selection ledger and sensitivity policy.
- [ ] `MEM-009` Pass restart, conflict, expiry, and cross-workspace isolation tests.
- [ ] `MEM-010` Implement explicit lifecycle and policy for working,
  conversational, episodic, semantic, preference, relationship, and procedural
  memory, including non-durable working state and confirmation rules by type.

## Milestone 5: Connectors

- [ ] `AUT-007` Implement the post-turn learning review as a durable workflow run:
  persisted before it produces any effect so it resumes after a crash, cancellable,
  bounded by a cumulative replayed-input-token budget that reports a budget stop,
  and producing typed candidates that reach the canonical learning tools only
  through the policy path. A review interrupted at any wait state recovers to an
  explicit resumable or failed state. See
  [ADR-0012](docs/adr/0012-governed-learning-loop.md).
Dependencies: Milestone 4 exit gate.

- [ ] `CON-001` Define connector manifest, lifecycle, auth, health, diagnostics,
  and quality-scale contracts.
- [ ] `CON-002` Implement OAuth/PKCE broker and secret-reference integration.
- [ ] `CON-003` Implement webhook verification, replay defense, inbox dedupe, and
  outbox handoff.
- [ ] `CON-004` Implement pagination, incremental sync, rate-limit, and retry
  primitives.
- [ ] `CON-005` Select Google or Microsoft as first vertical after current
  official-doc research and test-account readiness review.
- [ ] `CON-006` Implement first email search/read/draft workflow.
- [ ] `CON-007` Implement first calendar search/create workflow with approval.
- [ ] `CON-008` Implement GitHub App connector and webhook ingestion.
- [ ] `CON-009` Implement generic webhook reference connector.
- [ ] `CON-010` Enforce connector quality checklist in CI.

## Milestone 6: Automation

Dependencies: Milestone 5 exit gate.

- [ ] `AUT-001` Implement versioned event envelope and durable outbox/inbox.
- [ ] `AUT-002` Implement scheduler leases, clock abstraction, and misfire policy.
- [ ] `AUT-003` Implement native workflow definitions and persisted execution.
- [ ] `AUT-004` Implement retries, timers, event waits, approvals, cancellation,
  compensation, and restart recovery.
- [ ] `AUT-005` Implement proactive rules, quiet hours, budgets, and notification
  escalation.
- [ ] `AUT-006` Build crash-point and duplicate-delivery test harness.

## Milestone 7: Runtime Ecosystem

Dependencies: Milestone 3 exit gate and `TLS-009` for scoped runtime tool
credentials where MCP is used.

- [ ] `RTM-001` Define and version runtime handshake, capabilities, request, event,
  resume, cancellation, artifact, and error schemas.
- [ ] `RTM-002` Implement isolated runtime supervisor with limits and health.
- [ ] `RTM-003` Build runtime SDK and reference fixture runtime.
- [ ] `RTM-004` Research and implement OpenClaw adapter.
- [ ] `RTM-005` Research and implement OpenAI Agents adapter.
- [ ] `RTM-006` Research and implement ACP adapter for compatible coding agents.
- [ ] `RTM-007` Research and implement LangGraph adapter where graph semantics add
  value.
- [ ] `RTM-008` Test crash, hang, protocol mismatch, malformed events, and scoped
  tool access.
- [ ] `RTM-009` Decide the voice-agent-framework question on the runtime-adopter
  axis: complete the LiveKit evidence gate, verify SIP interoperability and
  live-audio turn-detection latency, determine whether the Rust transport crates
  can be built without a development runtime on the target machine, and either
  add a versioned adapter or record a reasoned rejection. Do not adopt framework
  tool, MCP, task, handoff, or fallback features as JARVIS implementations; they
  remain proposals validated against the canonical tool fabric.
- [ ] `RTM-010` Decide the realtime media platform question on the same
  runtime-adopter axis but as a **separate decision**: assess whether the
  candidate transport can be adopted as an external runtime and/or a replaceable
  media adapter with scoped grants and adapter-state separation from canonical
  session state, verify its self-hosting prerequisites, and either add a
  versioned adapter or record a reasoned rejection. This must not be merged with
  `RTM-009`: [ADR-0011](docs/adr/0011-realtime-media-session-boundary.md)
  records that the transport decision and the agent-framework decision are
  separate decisions on separate axes.

## Milestone 8: Voice

Dependencies: Milestones 2, 3, 4, and 6 exit gates. Telephony live tests also
require a dedicated test account, explicit spend approval, and applicable
consent/legal policy.

- [ ] `VOI-001` Define provider-neutral voice/call contracts and latency budgets,
  including the end-of-turn detection requirement and the separate turn-detection
  and generation measurements that compose the perceived budget.
- [ ] `VOI-002` Refresh ElevenLabs, Twilio, and LiveKit evidence before
  implementation, and record fixtures for the documented system-tool shapes and
  the `elevenlabs_extra_body`/session-binding placement.
- [ ] `VOI-003` Implement authenticated OpenAI-compatible Responses SSE endpoint.
- [ ] `VOI-004` Implement Chat Completions compatibility only where required.
- [ ] `VOI-005` Implement ElevenLabs Custom LLM inbound voice flow.
- [ ] `VOI-006` Implement ElevenLabs scoped MCP-client mode.
- [ ] `VOI-007` Implement verified post-call webhooks and call audit records.
- [ ] `VOI-008` Implement outbound call tool with consent, quiet hours, budgets,
  idempotency, callbacks, and legal-region policy.
- [ ] `VOI-009` Test interruption, transfer, end, voicemail, disconnect, duplicate
  callback, and no-double-ring behavior.
- [ ] `VOI-010` Implement an opt-in, step-up-authenticated voice approval channel
  that is denied by default and binds the exact JARVIS approval fingerprint.
- [ ] `VOI-011` Implement the provider-neutral call state controller for latency,
  interruption/barge-in, transfer, voicemail, partial transcripts, disconnect,
  timeout, callback ordering, bounded cleanup, and terminal reconciliation.
- [ ] `VOI-012` Implement the telephony carrier adapter with explicit TwiML
  attributes asserted in tests, single-point mode selection between the
  text-relay and raw-audio modes, webhook signature verification over raw bytes,
  callback deduplication, and geographic/spend controls.
- [ ] `VOI-013` Implement model-based end-of-turn handling: require an end-of-turn
  confidence threshold and a partial-transcript channel, keep partial text
  ephemeral, and measure turn-detection time separately from generation time.
- [ ] `VOI-014` Measure perceived turn latency on real calls per pipeline (text
  relay versus raw audio versus speech-to-speech) with components reported
  separately, and prove the recorded budget passes at p50 and p95.

## Milestone 9: Desktop

Dependencies: Milestone 1 exit gate for scaffolding. Each feature view depends
on its owning backend milestone; Milestone 9 cannot exit before Milestones 2
through 6 satisfy the contracts consumed by the desktop client.

- [ ] `UI-001` Research current Tauri v2 docs and refresh evidence.
- [ ] `UI-002` Scaffold Tauri/React client with generated API bindings.
- [ ] `UI-003` Implement capability-scoped IPC, CSP, and no direct secret access.
- [ ] `UI-004` Implement onboarding and daemon connection/recovery.
- [ ] `UI-005` Implement chat/activity and approval experiences.
- [ ] `UI-006` Implement memory, tasks, connections, tools, models, runtimes,
  voice, settings, and developer console.
- [ ] `UI-007` Implement signed updater and failed-update recovery.
- [ ] `UI-008` Run accessibility and desktop E2E tests on tier-1 platforms.
- [ ] `UI-009` Implement a device-bound mobile approval surface with step-up,
  notification expiry, revocation, and the same exact-action contract as CLI,
  API, and desktop.
- [ ] `MED-001` Define and version the media session contract: session identity,
  lifecycle states, participant and track records, grants, capture state,
  recording state, and terminal reconciliation, with the adapter's room/channel
  name never entering JARVIS state.
- [ ] `MED-002` Implement workspace-scoped session admission with JARVIS-issued
  credentials bound to principal, workspace, session, expiry, and role;
  track-granularity grants; and grant checks repeated at publication rather than
  only at admission. Transport-native permissions are defense in depth, never
  JARVIS authorization.
- [ ] `MED-003` Implement per-session capture consent: microphone, camera, screen,
  and screen-audio capture off by default, enabled only by an explicit visible
  user action, revocable immediately and recorded on the session, with platform
  capture limitations stated rather than hidden.
- [ ] `MED-004` Mediate the media data plane as policy input: text and byte
  streams, data tracks, participant attributes, and participant-to-participant
  method calls reach the canonical tool path only by producing a
  policy-evaluated intent, with schema-validated arguments and a fixed,
  non-extensible method set.
- [ ] `MED-005` Implement a provider-neutral media adapter behind the runtime
  protocol with scoped grants, adapter state kept separate from canonical session
  state, per-stage observability (transport connect, capture, perception,
  context, model, tool wait, first output; dropped frames, reconnects, denied
  grants, rejected messages), and removal of the adapter without loss of
  canonical session records, policy, tools, or CLI function.

## Milestone 10: Production Hardening

Dependencies: Milestones 1 through 9 exit gates for every feature included in
the production profile. Public release additionally requires `OWN-001` through
`OWN-005`.

- [ ] `PRD-001` Implement PostgreSQL/pgvector backend parity and migration tests.
- [ ] `PRD-002` Implement object storage and encrypted backup profiles.
- [ ] `PRD-003` Implement multi-user authentication, RBAC, service clients, quotas,
  and audit export.
- [ ] `PRD-004` Define and load-test SLOs and capacity limits.
- [ ] `PRD-005` Evaluate distributed bus/cache/workflow products from measurements;
  accept ADRs only when thresholds are exceeded.
- [ ] `PRD-006` Complete threat review, fuzzing, dependency/license review,
  penetration test, and incident exercise.
- [ ] `PRD-007` Verify data export/deletion, retention, key rotation, disaster
  recovery, rollback, and support procedures.
- [ ] `PRD-008` Implement and package the supported server-container mode with
  non-root execution, config/secret injection, migrations, readiness, drain,
  backup hooks, update, and lifecycle tests.
- [ ] `PRD-009` Run one crash-consistency, migration, compatibility, and rollback
  contract matrix across every durable aggregate and supported storage backend.
- [ ] `PRD-010` Enforce learning isolation and quota in the production profile:
  per-workspace learning grants, per-tenant review budgets, cross-workspace
  separation of learned skills and promoted memories under the server backend, and
  an audit export of learning decisions.

## Explicitly Deferred Until Evidence Justifies Them

- Redis
- NATS or Kafka
- Temporal as the default workflow engine
- Kubernetes
- Elasticsearch
- Qdrant or another dedicated vector database
- In-process native dynamic-library plugins
- A public plugin marketplace