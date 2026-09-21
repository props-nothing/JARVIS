# Integration Evidence: GitHub Actions CI

Status: ACCEPTED
Lifecycle: ACTIVE
Owner: Foundation
Last verified: 2026-09-21
Revalidate by: 2027-03-20
Implementation gate: PASSED

## Decision Summary

- Purpose: run the repository's engineering gates and produce the native
  per-target proof that the Foundation cannot obtain from a single authoring host.
- JARVIS boundary: CI is a build-and-test facility only. It is never a runtime
  dependency of `jarvisd` or `jarvis`, and no JARVIS code path branches on being
  in CI. Workflow files are part of the development control plane, not the product.
- Proposed package or protocol version: `actions/checkout@v7`, `actions/cache@v6`,
  `actions/setup-node@v7`, and the `ubuntu-24.04`, `ubuntu-24.04-arm`,
  `windows-2025`, `macos-14`, `macos-15-intel` runner labels.
- Supported deployment modes: hosted runners only.
- Explicitly unsupported: self-hosted runners, larger runners, Azure private
  networking, and static IPs. Larger runners would be an added trust boundary with
  networking configuration JARVIS does not need yet.
- Kill switch or disable path: delete or disable the workflow files. No product
  behavior depends on CI, so removing CI degrades evidence and never the product.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | `https://docs.github.com/llms.txt` | Auto-generated index | 2026-09-21 | The official index; it lists the runner and workflow-syntax articles used below |
| Runners reference | `https://docs.github.com/en/actions/reference/runners/github-hosted-runners` | Current | 2026-09-21 | Exact runner labels, CPU/RAM/architecture per label, and the administrative-privilege facts |
| Workflow syntax | `https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax` | Current | 2026-09-21 | `permissions`, `concurrency`, `timeout-minutes`, matrix `fail-fast`, `on` triggers |
| `actions/checkout` | `https://api.github.com/repos/actions/checkout/releases/latest` | `v7.0.1` (2026-07-20) | 2026-09-21 | The current major line for the checkout action |
| `actions/cache` | `https://api.github.com/repos/actions/cache/releases/latest` | `v6.1.0` (2026-06-26) | 2026-09-21 | The current major line for the cache action |
| `actions/setup-node` | `https://api.github.com/repos/actions/setup-node/releases/latest` | `v7.0.0` (2026-07-14) | 2026-09-21 | The current major line for the Node setup action |

Attempted `llms.txt` URLs that did not exist:

- `https://docs.github.com/en/actions/llms.txt` returned HTTP 404. The working
  index is the site-wide `https://docs.github.com/llms.txt`, which is what was
  used. Section-specific `llms.txt` files are not published for GitHub Actions.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| `actions/checkout` | `@v7` | `v7.0.1` | Verified: `@v7` resolves to the current major line |
| `actions/cache` | `@v6` | `v6.1.0` | Verified: `@v6` resolves to the current major line |
| `actions/setup-node` | `@v7` | `v7.0.0` | Verified: `@v7` resolves to the current major line |
| Runner labels | `ubuntu-24.04`, `ubuntu-24.04-arm`, `windows-2025`, `macos-14`, `macos-15-intel` | Documented labels | Verified against the runners reference |
| Node | `22` | LTS line | Used only for `node --test` and the smoke script; the product has no Node dependency |

**A mutable major tag is not a pinned version.** `@v7` is a moving alias, which is
the opposite of the exact-pin rule that applies to Rust dependencies. It is
accepted here because the actions are first-party GitHub tooling that performs no
JARVIS authorization or secret handling, and because a SHA pin would require a
matching evidence refresh on every upstream patch. If a JARVIS workflow ever gains
a signed-release or publication step, those steps must pin by full commit SHA —
that work is `FND-011` and must not inherit this decision.

## Contract

### Authentication and Authorization

- Credential type: none. Every workflow declares `permissions: contents: read`,
  which denies the default broad token scope.
- Credential placement: no secret is referenced by any workflow. There is no
  `secrets.*` expression and no environment secret.
- Required scopes: `contents: read` only.
- Refresh/rotation behavior: not applicable; no credential exists.
- Tenant or workspace binding: every job uses the explicit `ubuntu-24.04`,
  `ubuntu-24.04-arm`, `windows-2025`, `macos-14`, or `macos-15-intel` label
  rather than a `-latest` alias, so a runner image refresh cannot silently change
  the platform under test.
- Webhook signature verification: not applicable.

### Transport and Lifecycle

- Endpoint(s): none. CI only runs local commands.
- Transport(s): HTTPS to the action registry for action download, and to the Cargo
  registry for crates. No JARVIS network surface is exercised.
- Connection lifecycle: one job per gate; no long-running process.

### Failure Semantics

- `fail-fast: false` on the native matrix, so one broken platform does not mask
  the state of the others. A platform break is exactly the news this lane exists
  to deliver.
- `timeout-minutes` is set on every job (10 for docs, 30 for lints, 45 for tests,
  60 for native) so a hung job cannot consume the concurrency budget.
- A failing assertion and a missing tool are different outcomes and are not
  collapsed: the native lane verifies the C toolchain in its own step.

### Limits and Quotas

- `concurrency` cancels a superseded run for the same ref but never cancels a run
  on the default branch, so the branch of record always completes.
- Build caches are keyed by runner OS, target triple, and the `Cargo.lock` and
  `rust-toolchain.toml` hashes, with a narrower restore key. A single shared key
  would cross-contaminate two matrix entries, since two macOS and two Linux
  entries share a runner OS label family.
- Cache use is an optimization only: a cache miss must not change the result.
  Nothing is generated that is not reproducible from the repository.

## Security Analysis

- **Untrusted input.** A pull request can change workflow files. The workflows
  therefore request no write permission and reference no secret, so a malicious
  pull request has nothing to exfiltrate. `persist-credentials: false` on checkout
  keeps the job token out of the `.git/config` of the working tree, where a build
  script could read and reuse it.
- **`pull_request` versus `pull_request_target`.** Only `pull_request` is used.
  `pull_request_target` runs untrusted code with the base repository's token and
  secrets, which is the exact mistake this design avoids.
- **Third-party code execution.** Rust build scripts, `cc`, and the test suite all
  execute on the runner. That is inherent to compiling this workspace, and the
  mitigation is the absence of secrets and write permissions rather than
  sandboxing. The native lane deliberately does **not** register a real service,
  because that would mutate the runner's logon state and outlive the job.
- **Administrative privilege on the runner (verified fact).** The runners
  reference states that Linux and macOS runners use passwordless `sudo`, and that
  Windows runners run as administrators with UAC disabled. A test that behaves
  differently when it can elevate would therefore pass in CI while failing for a
  real unprivileged user. This is precisely why the service lane asserts the
  **absence** of elevation in the plan and why the service definition content is
  asserted rather than a real registration being attempted.
- **No publication.** No workflow uploads an artifact, publishes a package, or
  writes a release. Release artifacts and signing are `FND-011` and are blocked on
  `OWN-003`.

## Falsifiable Claims

| ID | Claim | Status | How it was checked | Failure it rules out |
| --- | --- | --- | --- | --- |
| `CI-C001` | The documented runner labels exist and match the tier-1 matrix | VERIFIED | Runners reference accessed and the labels compared to `installation-release.md` | A matrix entry silently runs on the wrong architecture |
| `CI-C002` | `aarch64` Linux has a standard hosted label | VERIFIED | `ubuntu-24.04-arm` is in the runners reference | The Linux aarch64 tier-1 target has no native lane |
| `CI-C003` | `aarch64` and `x86_64` macOS both have hosted labels | VERIFIED | `macos-14` and `macos-15-intel` are in the runners reference | The macOS tier-1 targets have no native lane |
| `CI-C004` | The action major versions in the workflows are current | VERIFIED | Release API for each action on 2026-09-21 | A workflow references a deprecated or nonexistent action version |
| `CI-C005` | No workflow references a secret or requests write permission | VERIFIED by inspection | Every job declares `contents: read` and no `secrets.*` appears | An untrusted pull request exfiltrates a credential |
| `CI-C006` | A pull request cannot obtain write access or a released credential | VERIFIED | `permissions: contents: read`, `pull_request` only, `persist-credentials: false` | `pull_request_target`-style token abuse |
| `CI-C007` | The clean-machine journey passes from an empty profile directory | VERIFIED live on Windows | `node scripts/clean-machine-smoke.mjs target/debug` | The journey depends on ambient profile state |
| `CI-C008` | The daemon becomes ready against a profile created by a previous run | VERIFIED live on Windows | The smoke script restarts the daemon against the same profile | A restart depends on a clean directory, or state is lost |
| `CI-C009` | The smoke journey leaves no daemon behind when it fails | VERIFIED by construction | `finally` block terminates the child and removes the profile | A leaked daemon poisons a later step |
| `CI-C010` | The Unix owner-only permission assertions actually execute somewhere | **FALSE as first recorded, now VERIFIED locally on Linux** — the native lane never ran (see `CI-C013`) and its command was invalid; `cargo test -p jarvis-infrastructure paths::` now runs 8 tests including `created_directories_are_owner_only` and `a_group_readable_directory_is_flagged_unsafe` | `cargo test -p jarvis-infrastructure paths::` on a Linux runner; confirmed in a `rust:1.98-slim-bookworm` container | `#[cfg(unix)]` assertions are typechecked but never run, as they are on the authoring host |
| `CI-C011` | A native target builds with its own C toolchain and no cross-compilation | VERIFIED by workflow construction | Host target equals matrix target; the C compiler is checked in its own step | A "supported target" that has never actually been built |
| `CI-C012` | A platform failure is visible rather than masked | VERIFIED | `fail-fast: false` | One broken platform hides the state of the other four |
| `CI-C013` | The workflows have actually run on GitHub's infrastructure | **VERIFIED GREEN at `7a7d61d`** — `CI` = success and `Native targets` = success with **all five tier-1 jobs green**. Before the fixes below, 12 runs existed and all 12 had failed | `GET /repos/props-nothing/JARVIS/actions/runs` and `.../runs/<id>/jobs` | A workflow that is syntactically valid but fails on a real runner |
| `CI-C017` | Every tier-1 target builds, tests, and passes its journeys on native hardware | **VERIFIED GREEN**: `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` all `success` | Run `35657958359` job list | A "supported" target that has never run its own tests |
| `CI-C015` | The native lane produces jobs at all | **FALSE until fixed.** `native.yml` had a YAML parse error, so GitHub created runs with **zero jobs** and the run name fell back to the file path instead of `Native targets` | The runs API reported `jobs=0`; `js-yaml` reproduced it locally | An entire lane that silently does nothing while reporting a failure |
| `CI-C016` | The lane can observe defects the Windows authoring host cannot | **VERIFIED after fixing.** Linux clippy failed on an unused import, an unused `mut`, and a `verbose_bit_mask` lint, and the clean-machine journey failed on a quote-dependent assertion | `rust:1.98-slim-bookworm` container with the pinned toolchain | Three real defects that passed every local Windows check |
| `CI-C014` | Every document is reachable from the top-level index | VERIFIED by validator and a fail-closed test | `validateIndexCompleteness` in `scripts/validate-docs.mjs`, falsified with a temporary unlinked file | A document nobody links to, which is effectively unreviewed |

`CI-C010` was false when first recorded and is now true. It claimed the Unix
owner-only permission assertions executed in the native lane; the lane never ran,
and its command was an invalid `cargo` invocation that would have matched nothing
had it run. Both are fixed, and the assertions are confirmed executing on Linux.

`CI-C013` and `CI-C015` were also worse than recorded. The note said the repository
was private and the runs API unreadable; it is **public**, the API returned the runs
without a token, and **all 12 runs had failed**. The native lane was worse than
failing: a YAML parse error meant it produced **zero jobs**, so it reported a failure
with no step to inspect and nothing ever executed in it.

The fixes came in three rounds, and each defect was only visible once the previous
one was fixed — a lane that cannot parse reports nothing about its contents:

1. **The YAML parse error.** `paths:: storage::` unquoted, where `: ` is a mapping
   separator in a plain scalar. Quoting the `run:` value fixed it, and the lane
   immediately produced jobs for the first time.
2. **The invalid command and the Linux-only clippy failures.** `cargo test` accepts
   exactly one positional filter, so `paths:: storage::` was a usage error; and
   Linux clippy found an unused import (of a constant used only under `not(unix)`),
   an unused `mut` in a `#[cfg(unix)]` block, and a `verbose_bit_mask` lint that
   only compiles where the Unix permission code does.
3. **A macOS-only journey failure.** The service assertion matched `ExecStart=`,
   which is a systemd directive; launchd renders the executable path inside a
   `<string>` element. It now matches the executable **path**, because all three
   backends must name it and a path is platform-independent where a directive name
   is not. `scripts/service-assertion-check.mjs` asserts the matcher against all
   three real renderings plus two negative cases, which is how a platform-specific
   assertion is caught without needing that platform.

A working Linux environment was the missing capability for round 2: the authoring
host has no Unix runtime, so `#[cfg(unix)]` code was never compiled, let alone
linted or tested. A `rust:1.98-slim-bookworm` container with the pinned toolchain,
and `scripts/linux-verify.sh`, reproduce the CI environment locally. No amount of
re-reading Windows-only output could have found those defects: they are invisible to
a compiler that never sees the code.

`CI-C014` was added after this audit found the top-level index had drifted twice:
a document was added, linked from its own section index, and never linked from
`docs/README.md`. Both omissions are now fixed, and the new check makes the
failure mechanical rather than something a reader has to notice.

## Verification Plan

1. Push to a branch and confirm the `CI` workflow's three jobs pass.
2. Confirm the `Native targets` matrix runs all five entries and that a
   deliberately broken entry fails without cancelling the others.
3. Confirm from a job log that the Unix permission tests executed on the three
   non-Windows lanes (they are skipped on the authoring host).
4. Confirm the clean-machine journey step ran on each native lane.
5. Re-check the action major versions quarterly, or when a workflow is edited.

## Operational Readiness

- A failing gate names the exact command in its step name, so a failure is
  actionable without reading the raw log.
- The native lane prints `rustc --version --verbose`, `cargo --version`, and the
  operating-system identity, so an evidence note can cite the exact environment a
  result came from.
- Skipped tests are reported as residual risk, never as success. A lane that did
  not run is not a lane that passed.

## Open Questions

- Whether to pin the actions by commit SHA. Deferred: it matters once a workflow
  handles signing or publication (`FND-011`), not for read-only gates.
- Whether the native matrix should also build the desktop bundle. Not yet: the
  desktop surface does not exist.
- Whether the release-profile smoke journey is worth its build time on every push,
  or should be scheduled. Currently it runs on every push for the native lanes
  only.

## Change Log

| Date | Change | Why |
| --- | --- | --- |
| 2026-09-21 | Initial note; `CI` and `Native targets` workflows added; clean-machine smoke journey added | `FND-010`: no CI existed, and the Unix permission assertions and per-target builds had never executed anywhere |
| 2026-09-21 | Action majors corrected to `checkout@v7`, `cache@v6`, `setup-node@v7` | The release API showed the initially written `@v5`/`@v4` pins were outdated |
| 2026-09-21 | Post-push audit: `CI-C013` reworded, `CI-C014` added, index drift fixed | The workflows were committed and pushed (commit `930e1d5`), so "never executed" was no longer accurate. **This audit also asserted the repository was private and the runs API unreadable; that was wrong** — see the entry below |
| 2026-09-21 | **Real CI results read for the first time.** Four defects found and fixed: a YAML parse error that made the native lane produce zero jobs; an invalid two-filter `cargo test` command; three Linux-only clippy failures; and a platform-dependent journey assertion. `CI-C010` corrected from VERIFIED-in-definition to FALSE-as-recorded; `CI-C015` and `CI-C016` added; `CI-C013` corrected | The repository is public, the runs API IS readable without a token, and **every one of the 12 runs had failed**. The previous note's "unverified" framing understated it: the native lane never executed anything at all. A `rust:1.98-slim-bookworm` container and `scripts/linux-verify.sh` now make the Unix-only defects reproducible |
