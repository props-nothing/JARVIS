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
  terminal states stay one-to-one. **The authenticated identity is an extractor** —
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
  populate.
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
  **Still not done:** the exception lifecycle (issue/expire/revoke/single-use) has a table and no
  code, so `ModelRouteDecision::exception_ref` is always absent and no route can relax a hard
  rule; the inventory attaches no region, retention, or training-use evidence until `BRN-011`
  measures capabilities, which makes a policy demanding **documented** evidence refuse every
  candidate rather than having a claim inferred for it; and the create-run policy reference is
  still unread — now **possible** to wire rather than blocked, because the `PUT` is what lets an
  operator create a policy, but still a separate increment since every existing E2E harness
  submits a policy that has never existed.
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
- [ ] `BRN-012` Persist the failure code on a terminal run transition, so a failed run's own
  row explains its outcome. Found while adding the policy-ceiling test above:
  `agent_runs.error_code` exists, is selected by `run_columns!()`, is read into
  `StoredRun::error_code`, and is **never written** — the transition `UPDATE` in
  `jarvis_infrastructure::storage::repositories` sets `state`, `version`, `completed_at`,
  `waiting_kind`, and `waiting_ref` only. The consequence is that `GET /api/v1/runs/{id}` reports
  `error_code: None` for a run that failed, and the recovery pass cannot distinguish "interrupted"
  from "failed for a reason" without reading the activity events. Closing it needs the code to
  travel on `RunTransition`/`RunWrite` from the controller's outcome rather than being scraped
  from the event's reason string, plus a migration-free adapter change. The test in
  `run_controller/tests.rs` asserts the current `None` **with a comment naming the gap**, so the
  value is pinned and visible rather than silently tolerated.
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