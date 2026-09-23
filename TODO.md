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
- [ ] `BRN-003` Research and implement one OpenAI-compatible provider adapter.
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
  resource surface and the CLI chat path are implemented and verified end to end
  against a real daemon; the **live** SSE follow is not, and is named below.
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
  the events endpoint delivers the **retained** public events and closes, so a client
  follows a run by reconnecting with `Last-Event-ID` until a terminal event arrives — it
  does **not** hold the connection open and push each new event as it is published. A
  streaming response body needs a `Stream` implementation, and this crate has neither a
  stream crate nor `axum`'s `sse` feature in its reviewed dependency set; adding either
  is a dependency change the integration research gate requires evidence for, and
  hand-writing a `Stream` would be an unreviewed async state machine on the
  security-relevant path. Also not done: the CLI has no streaming renderer that prints
  deltas as they arrive from one connection, `Idempotency-Key` is scoped per client
  rather than per principal-and-credential as the contract words it, the run list and
  the `jarvis ask` conversation-continuation option are absent, and the E2E step's
  abrupt-restart and disconnect cases belong to `BRN-008`.
  **The golden-fixture half of contract test 12 is now done, and it found two defects.**
  `jarvis-protocol`'s `run_contract_tests` and `jarvis-domain`'s
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
  three enforcement tests fail. **Not done:** usage is not yet summed **across** a run's
  calls (there is one call today, so the ceilings are per-call in effect), there is no turn
  budget, no byte or concurrency budget, no retry budget, and the disconnect case remains
  open — so `ACC-073` is closer but not closed.
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
  routes (`BRN-003`, `BRN-010`), so the contract's fallback list is not implemented; the retry
  policy is not settable per request (it is a field on the run's budget, defaulting to no
  retry, and `CreateRunRequest` has no typed override); and the disconnect case remains open.
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
- [ ] `BRN-009` Add deterministic orchestration tests and gated provider smoke test.
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
  **Not done, and named:** `first_output_at` is recorded by the controller and read by the adapter
  with **no producer of the instant** yet, because nothing in this build measures
  time-to-first-token — that measurement is `BRN-011`'s subject, so the honest state is a wired
  column and an absent producer rather than a dressed-up feature.
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
    cleanly** rather than leave a half-wired field.
  946 workspace tests. **DO NOT COMMIT.**
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
    adjusted.** `a_grant_needing_step_up_is_refused_at_standard_assurance` set the request's
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
    `the_status_example_names_the_capabilities_the_daemon_actually_serves` and its comment said "the
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
  - **The test-shaped defect first:** `the_double_enforces_scope_like_the_real_adapter` claimed in its
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
- [ ] `BRN-011` Measure and record incremental-delivery capability per model
  (time to first token **and** chunk spread) rather than a streaming boolean, and
  fail a route selection when a pinned model reports streaming but delivers its
  output in one burst.

## Milestone 3: Tool Fabric

Dependencies: Milestone 2 exit gate.

- [ ] `TLS-001` Define canonical tool schema, identity, origin, effects, scopes,
  risk, timeout, and retry metadata.
- [ ] `TLS-002` Implement registry discovery independently from grants.
- [ ] `TLS-003` Implement input/output validation and bounded result storage.
- [ ] `TLS-004` Implement deterministic policy evaluation and explainable decisions.
- [ ] `TLS-005` Implement durable approval records and action fingerprinting.
- [ ] `TLS-006` Implement idempotent tool-call ledger and execution state machine.
- [ ] `TLS-007` Implement safe reference filesystem read and write-plan tools.
- [ ] `TLS-008` Refresh MCP evidence; implement stdio and Streamable HTTP client.
- [ ] `TLS-009` Implement scoped authenticated MCP server export.
- [ ] `TLS-010` Add MCP negotiation, auth, cancellation, malformed payload, and
  conformance/Inspector tests.
- [ ] `TLS-011` Define plugin manifest and process supervision contract.
- [ ] `TLS-012` Prove native/MCP/runtime routes cannot bypass policy.
- [ ] `TLS-013` Implement authenticated approval list, preview, decide, expire,
  revoke, and resume use cases for API and CLI with channel assurance checks.
- [ ] `TLS-014` Implement plugin package provenance/signature verification,
  compatibility validation, install-disabled, staged update/rollback, disable,
  data-retention choice, and removal.
- [ ] `TLS-015` Implement plugin grants, scoped launch environment, process
  supervision, resource limits, health, crash-loop quarantine, and audit.

## Milestone 4: Memory

Dependencies: Milestone 3 exit gate.

- [ ] `MEM-001` Define typed memory and provenance schema.
- [ ] `MEM-002` Implement user-confirmed preference memory lifecycle.
- [ ] `MEM-003` Implement lexical retrieval and deterministic ranking baseline.
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

## Explicitly Deferred Until Evidence Justifies Them

- Redis
- NATS or Kafka
- Temporal as the default workflow engine
- Kubernetes
- Elasticsearch
- Qdrant or another dedicated vector database
- In-process native dynamic-library plugins
- A public plugin marketplace