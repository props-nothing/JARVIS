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
| `CI` | `docs` | The documentation, requirement, TODO, contract, and evidence structures are consistent |
| `CI` | `lint` | `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `CI` | `test` | `cargo test --workspace --all-features` on Linux x86_64 |
| `Native targets` | `native` | Per-target build, test, and clean-machine journey on each tier-1 target |

The `docs` lane runs `node --test scripts/validate-docs.test.mjs` **before**
`node scripts/validate-docs.mjs`. The order matters: a weakened validator must not
be able to pass the gate it implements.

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
typechecked on Windows but never executed there, so the native lane runs them
explicitly:

```bash
cargo test -p jarvis-infrastructure --all-features paths:: storage::
```

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
6. assert `jarvis service show` names `jarvisd` and states the per-user mode.

It asserts directory contents, not only exit codes, because an exit code of 0
with nothing written would still be a failure. It terminates the daemon in a
`finally` block so a failed assertion cannot leak a process into a later step.

The journey deliberately does **not** register a real service. Hosted runners are
administrators with UAC disabled on Windows and have passwordless `sudo` on Unix,
so a real registration would both mutate runner logon state and risk passing
under privileges a real user does not have.

## Running The Gates Locally

```bash
node --test scripts/validate-docs.test.mjs
node scripts/validate-docs.mjs
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build -p jarvisd -p jarvis-cli
node scripts/clean-machine-smoke.mjs target/debug
```

The changed-path gate applies when a change touches an external integration path:

```bash
node scripts/validate-docs.mjs --changed-file <path> [--changed-file <path> ...]
```

Pass the evidence note and `docs/research/evidence-manifest.json` in the same
invocation as any changed dependency manifest, or the gate refuses the change.

## Limitations And Residual Risk

- **The workflows have not run on GitHub's infrastructure.** They are syntactically
  valid and the journey passes locally on Windows, but the first real run is
  unproven. This is recorded as `CI-C013`, `UNVERIFIED`.
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
