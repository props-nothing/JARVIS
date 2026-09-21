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
  status|update|rollback|uninstall`). The install root and the user profile are
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

- [ ] `BRN-001` Define domain model/provider capabilities and normalized model
  stream contract.
- [ ] `BRN-002` Implement deterministic scripted model provider for tests.
- [ ] `BRN-003` Research and implement one OpenAI-compatible provider adapter.
- [ ] `BRN-004` Implement durable session/message/run/model-call repositories.
- [ ] `BRN-005` Implement native agent state machine with explicit terminal and
  waiting states.
- [ ] `BRN-006` Implement context budgeting for identity, active task, and recent
  conversation.
- [ ] `BRN-007` Implement CLI chat plus HTTP/SSE streaming.
- [ ] `BRN-008` Implement cancellation, timeout, disconnect, fallback, and daemon
  restart behavior.
- [ ] `BRN-009` Add deterministic orchestration tests and gated provider smoke test.
- [ ] `BRN-010` Implement a visible, configurable model data-use, retention,
  locality, and telemetry policy that constrains routing and records provider
  disclosures/effective decisions.
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