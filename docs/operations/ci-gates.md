# CI Gates and Native Lanes

Status: ACCEPTED
Owner: Foundation
Last verified: 2026-09-21

This is the operator-facing description of what CI proves and, equally important,
what it does not. The evidence note with the researched versions and falsifiable
claims is
[GitHub Actions CI](../research/integrations/github-actions.md).

## What Runs On Every Push

| Workflow | Lane | What it proves |
| --- | --- | --- |
| `CI` | `docs` | The documentation, requirement, TODO, contract, and evidence structures are consistent, and the service assertion holds on every platform's rendering |
| `CI` | `lint` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo doc --workspace --no-deps --all-features` |
| `CI` | `test` | `cargo test --workspace --all-features` on Linux x86_64 |
| `Native targets` | `native` | Per-target build, test, and clean-machine journey on each tier-1 target |

The `docs` lane runs `node --test scripts/validate-docs.test.mjs` **before**
`node scripts/validate-docs.mjs`. The order matters: a weakened validator must not
be able to pass the gate it implements.

The `lint` lane's `cargo doc` step is what makes the crate-level
`#![deny(rustdoc::broken_intra_doc_links)]` real. `clippy` compiles doc comments
without resolving their links, so without a step that runs rustdoc, a doc link
pointing at a symbol that does not exist is **indistinguishable from one that
resolves** — `BRN-035` found 21 such links, two of them naming a type that was never
written anywhere in the workspace. The step deliberately does **not** pass
`-D warnings`, which would additionally deny `redundant_explicit_links` and
`private_intra_doc_links` — style complaints about links that *do* resolve — and
widening a gate past the defect it was added for is how a gate gets relaxed later.

## The Service Assertion Guard

`scripts/daemon-assertion.mjs` owns the single definition of "this service preview
names the daemon executable". The matcher is defined once and imported by both
users, so the guard and the journey it guards cannot test different regexes:

- `scripts/clean-machine-smoke.mjs` applies it to the preview produced on the
  running platform. This is the assertion that matters, and it runs on all five
  tier-1 targets.
- `scripts/service-assertion-check.mjs` applies it to the recorded rendering of
  all three per-user backends plus two negative cases. The `docs` lane runs it,
  because a platform-specific matcher is otherwise only observable on the platform
  it breaks.

The assertion has been wrong twice in the same way — it keyed on one platform's
formatting (`ExecStart=`, a systemd directive). It passed on Windows and Linux and
failed on macOS, where launchd renders the path inside a `<string>` element. The
match is on the executable **path**, which every backend must contain, rather than
on a directive name, which they do not share.

Two negative cases carry the weight. A preview naming only the client would start
and never serve. A *description* containing the word `jarvisd` in quotes is the
case that made the first version pass on Windows for the wrong reason: a bare-name
assertion is satisfied by text that is not the executable.

## The Native Matrix

| Target | Runner | Why |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `windows-2025` | Tier-1 Windows |
| `aarch64-apple-darwin` | `macos-14` | Tier-1 macOS arm64 |
| `x86_64-apple-darwin` | `macos-15-intel` | Supported while Apple supports it |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | Tier-1 Linux |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | Tier-1 Linux arm64 |

`fail-fast: false` is deliberate. If one platform breaks, the other four must
still report, because a platform break is the information this lane exists to
produce.

## Why Native Runners Rather Than Cross-Compilation

Bundled SQLite compiles a vendored `sqlite3.c` through `cc`. A build for a
non-host target needs that target's C compiler and linker, so this workspace does
**not** cross-compile from a single host. Each supported target therefore has to
prove itself on matching hardware. "Rust can compile for it" is not platform
support.

## What Runs Only On Unix Runners

The owner-only permission assertions live in `#[cfg(unix)]` tests. They are
typechecked on Windows but never executed there, and the full workspace suite on a
Linux runner already compiles and runs them. The native lane also names them
explicitly, so a regression in `paths::` is attributable from the log rather than
by reading thousands of test lines:

```bash
cargo test -p jarvis-infrastructure --all-features paths::
```

Two details here are each a real bug when got wrong, and both were found by
reading the first real CI results rather than by reasoning:

- `cargo test` accepts exactly **one** positional filter. `paths:: storage::` is a
  usage error rather than two filters, so the step ran no tests and failed the
  lane with `unexpected argument 'storage::'`.
- The argument must be quoted in YAML. Unquoted, the `: ` (colon then space) is
  read as a mapping separator inside a plain scalar, the workflow file fails to
  parse, and GitHub then creates a run with **zero jobs** — a failure with no step
  to inspect, which is why this looked like a code problem for several commits.

Until that step runs in CI, those assertions are typechecked only, and the
Foundation TODO records them as such rather than as proven.

## The Clean-Machine Journey

`scripts/clean-machine-smoke.mjs` does what `ACC-001` describes using only the two
built binaries and an empty directory:

1. start the daemon against a fresh `--profile` directory and wait for readiness;
2. run `jarvis status` and `jarvis doctor` and require no blocking findings;
3. assert the profile actually contains the credential, database, discovery file,
   and lock, and that the credential is mode `0600` on Unix;
4. stop the daemon and assert it drained within its bound;
5. start it again against the same profile and assert readiness and a clean
   doctor report;
6. assert `jarvis service show` names the daemon executable and states the
   per-user mode, using the shared matcher described above;
7. run `jarvis repair --confirm` and require it to converge to a state with
   nothing left to fix, then kill the daemon uncleanly and require `doctor` to
   report `jarvis.stale_discovery` and `repair` to clear it while leaving the
   benign lock file in place;
8. preview a support bundle (requiring that nothing was written), then export one
   and require it to omit a credential planted in a log file.

Step 7 is what makes the daemon-state repair credible: the discovery file is
required to have **survived** the unclean kill before `doctor` is asked to detect
it, so the assertion cannot pass vacuously. The same kill also exercises the
probed reachability, because a stale file with nothing behind it must report
`jarvis.daemon_unreachable` rather than a running daemon.

It asserts directory contents, not only exit codes, because an exit code of 0
with nothing written would still be a failure. It terminates the daemon in a`finally` block so a failed assertion cannot leak a process into a later step.

The journey deliberately does **not** register a real service. Hosted runners are
administrators with UAC disabled on Windows and have passwordless `sudo` on Unix,
so a real registration would both mutate runner logon state and risk passing
under privileges a real user does not have.

## The Release Verification Journey

`scripts/release-verify-smoke.mjs` proves the consumer side of `FND-011`: that a
release a user downloads is the release that was signed, and that a modified byte
is refused. It uses only the two built binaries, the committed
**non-production test key**, and a temporary directory. It publishes nothing.

1. stage the built binaries as a release directory under their release names;
2. build and sign a manifest over their real digests with the test key;
3. require `jarvis verify-release` to **succeed** and to disclose the test key;
4. flip one artifact byte and require it to **fail** with
   `jarvis.release_digest_mismatch`;
5. rewrite a digest in the manifest body and require it to **fail** with
   `jarvis.release_signature_mismatch`;
6. present a signature naming an untrusted key and require it to **fail** with
   `jarvis.release_key_unknown`;
7. restore the genuine bytes and require success again.

Step 7 is what makes the three refusals credible. Without it, a journey whose
fixture never verified in the first place would report the same failures and look
like a pass. A verifier that accepted everything would satisfy a
verify-a-good-release check, which is why every step after the first requires a
failure rather than a success.

**No production identity is involved, and none exists** (`OWN-003`). The signing
key is committed, has no trust outside the built-in store, and signs only
artifacts whose build identity is prefixed `non-production-test/`. A workflow
that ever handles production signing or publication must pin its actions by full
commit SHA rather than the mutable major tags the read-only lanes use; see the
[release signing evidence note](../research/integrations/release-signing.md).

## The Install Journey

`scripts/install-smoke.mjs` proves the installed-version lifecycle (`FND-012`). It
uses only the built binaries, the committed test key, and a temporary root, and it
publishes and registers nothing:

1. install 0.1.0 from a verified release and require it to become active;
2. assert the binary is staged under its **stable installed name**, not the
   version-named download, because a service definition points at the stable name;
3. seed user data that every later step must preserve;
4. update to 0.2.0 and require the previous version to be recorded;
5. remove the rollback target and require the rollback to be **refused**, and
   require the refusal to leave the active version unchanged;
6. restore the target and roll back, requiring the old version to be active and the
   new version to still be installed;
7. uninstall and require the program files gone while the database is present and
   **byte-identical**;
8. refuse a purge without `--acknowledge-purge`, then prove an acknowledged purge
   removes the data it named.

Steps 5 and 8 carry the weight. A rollback that quietly did nothing, and a purge
reachable through the same single flag as a safe uninstall, are the two failure
modes this journey exists to catch. Step 7 asserts the database *contents*, because
an uninstall that left an empty file behind would pass a presence check while still
destroying user data.

## Reproducing The Linux Lanes Locally

The authoring host is Windows, so `#[cfg(unix)]` code is never compiled there and
Unix-only defects are invisible to `cargo clippy` and `cargo test`. Four CI
failures were exactly that class, and none of them could have been found by
re-reading Windows output.

`scripts/linux-verify.sh` runs the lane's commands in a Linux container with the
pinned toolchain, which makes those defects reproducible:

```bash
docker run --rm -v "$PWD:/src" -w /src rust:1.98-slim-bookworm sh /src/scripts/linux-verify.sh
```

It runs `cargo fmt --check`, `clippy -D warnings`, the workspace tests under the
workflow's `RUSTFLAGS`, the `paths::` filter that covers the Unix permission
assertions, a release build, and all three journeys. Its value is that it is
**differently blind** from the host: it found an unused import that only exists
under `not(unix)`, an unused `mut` that only exists under `unix`, and a clippy lint
that only compiles where the permission code does.

It is a developer aid, not a gate: CI remains the authority, because only CI proves
that the workflow file itself parses and that the jobs actually run.

## Running The Gates Locally

```bash
node --test scripts/validate-docs.test.mjs
node scripts/validate-docs.mjs
node scripts/service-assertion-check.mjs
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps --all-features
cargo test --workspace
cargo build -p jarvisd -p jarvis-cli
node scripts/clean-machine-smoke.mjs target/debug
node scripts/release-verify-smoke.mjs target/debug
node scripts/install-smoke.mjs target/debug
```

The changed-path gate applies when a change touches an external integration path:

```bash
node scripts/validate-docs.mjs --changed-file <path> [--changed-file <path> ...]
```

Pass the evidence note and `docs/research/evidence-manifest.json` in the same
invocation as any changed dependency manifest, or the gate refuses the change.

## Limitations And Residual Risk

- **`docs/research-telephony/` is excluded from the repository and from the docs
  gate, and the durable findings have been promoted, so the folder is now
  disposable and absent from a clean checkout.** It was a telephony/LiveKit spike
  with its own Node toolchain, lockfile, and a nested `.gitignore` protecting its
  `.env`; it is not part of the JARVIS control plane. Nothing in it was ever
  tracked. Two protections keep it from coming back unnoticed:
  `.gitignore` excludes the whole directory so a broad `git add` cannot publish
  it or its `.env`, and the docs validator skips it explicitly so its Markdown
  cannot satisfy a status or index check and a scratch file cannot change the
  validated file count.
  Two consequences are worth stating plainly:
  **a repository ignore rule is not a security control** — the `.env` there was
  protected only by the file not being added, so if a secret ever passed through
  it, it should be rotated rather than merely untracked; and the exclusion is a
  visible, reviewable entry in both `.gitignore` and the validator rather than a
  `--force`-able convention.
  **The folder may be deleted, and the gate must not depend on it existing.**
  The measured latency baseline, the carrier attribute table and mode-exclusivity
  trap, and the framework review live in
  [the Twilio telephony note](../research/integrations/telephony-twilio.md) and
  [the LiveKit note](../research/integrations/livekit.md), with the sources
  registered in `source-registry.md` and the entries added to
  `evidence-manifest.json`. Re-capture any fixture that a promoted note claims as
  `OBSERVED` before relying on it, because a note is not a substitute for the raw
  capture. The exclusion probe in `scripts/validate-docs.test.mjs` **creates the
  directory it needs and removes only what it creates**, because a test that
  required the scratch folder to exist made deleting a disposable folder a red
  build — the test now fails only if the `EXCLUDED_DIRECTORIES` entry is removed,
  which is the behavior it is meant to protect.
- **The workflow results are not verified by any document here.** The lanes are
  being triggered; read the Actions tab, and read the `Native targets` matrix in
  particular, since it is the only place the Unix permission assertions and four of
  the five target builds have ever executed.
- **Action pins are mutable major tags.** `@v7` is a moving alias, unlike the exact
  pins required of Rust dependencies. This is accepted for read-only gates and must
  not be inherited by any future signing or publication step, which must pin by
  commit SHA.
- **The runner platform is not the user's platform.** A runner image is a clean
  container or VM, not a user's machine with their existing software, antivirus,
  proxy, or corporate policy. Clean-machine CI is necessary but not sufficient;
  packaged-install acceptance on real hardware remains required.
- **No packaged artifact is exercised yet.** The journey runs the release-profile
  binaries directly from `target/release`, not from an installer or archive.
  Packaged journeys are `FND-011` and `FND-012`.
- **Nothing is signed, published, or promoted.** No workflow uploads an artifact or
  writes a release, and none references a secret.
