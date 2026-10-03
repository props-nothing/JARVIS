# Integration Evidence: Capability-Based Filesystem Access (`cap-std`)

Status: ACCEPTED
Owner: Tool fabric (`TLS-007`)
Last verified: 2026-10-03
Revalidate by: 2027-01-01
Implementation gate: PASSED
Review scope: The `cap-std` 4.0.3 crate (README, SECURITY policy, crates.io metadata, licences of the new
transitive packages) as the enforcement layer under the native `files.list@1`, `files.read@1` and
`files.write@1` tools. Gate PASSED for those three tools over operator-declared roots on the platforms in
"Version Matrix"; symlink behaviour on Windows and the macOS/Linux builds stay `UNVERIFIED` and are named in
"Open Questions". The shell, network, and process tools are **not** covered by this note.

## Decision Summary

- Purpose: let the model read and write files **only inside directories the operator declares**, with the
  containment enforced by the operating system handle rather than by string checks on a path.
- JARVIS boundary: `jarvis-infrastructure::native_tools::files` owns the tools. The domain
  (`WorkspaceRelativePath::denial_for`) refuses a path by *shape* (absolute, `..`, drive, NUL, backslash
  tricks) before anything touches disk; `cap-std` is the second, structural layer. Neither is trusted alone.
- Proposed package: `cap-std = "=4.0.3"`, `default-features = false`.
- Supported deployment modes: local daemon on Windows, Linux, macOS. Roots are configuration
  (`[[tools.files.roots]]`), so a profile with none exposes **no** filesystem tool at all.
- Explicitly unsupported: creating directories, deleting, renaming, binary content, following a symlink or
  junction out of a root, any path the operator did not declare.
- Kill switch or disable path: remove the roots from the profile and restart; the tools are no longer
  offered and a call to one is refused `NotFound`. Per-call, the standing approval is revocable
  (`approvals cancel`) and the `[[tools.deny]]` rules apply to these capabilities like any other.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| Repository and README | <https://github.com/bytecodealliance/cap-std> | tags v4.0.3, v4.0.2, v4.0.0; pushed 2026-08-20; not archived | 2026-10-03 | A capability-based `std::fs` replacement: a `Dir` is the authority; it protects against CWE-22, refuses to follow symlinks that leave the sandbox, and supports Linux, macOS, FreeBSD and Windows. It states it is **not** a sandbox for untrusted Rust code |
| crates.io metadata | <https://crates.io/crates/cap-std> | 4.0.3 is the newest stable; 4.0.1 is yanked; 3.4.6 is the newest 3.x | 2026-10-03 | The version pinned here is not yanked; default features are empty (`fs_utf8` and `arf_strings` are opt-in) |
| Security policy | <https://github.com/bytecodealliance/cap-std/blob/main/SECURITY.md> | main | 2026-10-03 | A reporting channel exists; the project treats sandbox escapes as vulnerabilities |
| API documentation | <https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html> | 4.0.3 | 2026-10-03 | `Dir::open_ambient_dir`, `open_with`, `read_dir`, `OpenOptions` (`create_new`) |

Attempted `llms.txt` URLs that did not exist:

- `https://docs.rs/llms.txt` → **HTTP 400**; `https://docs.rs/cap-std/llms.txt` → **HTTP 400**
- `https://github.com/bytecodealliance/cap-std/llms.txt` → **HTTP 404**
- `https://bytecodealliance.org/llms.txt` → **HTTP 404**
- `https://raw.githubusercontent.com/bytecodealliance/cap-std/main/llms.txt` → **HTTP 404**

The provider publishes no `llms.txt`, so the README, crates.io record, security policy and docs.rs pages above
are the substitute, as `rfc8785-canonicalization.md` and `release-signing.md` record for their sources. There is
no `CHANGELOG` or `RELEASES` file in the repository; release notes are the GitHub tags.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| `cap-std` | `=4.0.3`, no default features | 4.0.3 | VERIFIED: builds and passes on Windows with the pinned toolchain |
| Windows containment | NTFS directories and directory junctions | README: Windows supported | VERIFIED for **junctions** by `a_directory_junction_that_points_outside_the_root_is_not_followed`; symlink escape `UNVERIFIED` (the test account has no symlink privilege and the test skips itself) |
| Linux and macOS | `openat2`/`O_NOFOLLOW`-style resolution inside `cap-primitives` | README: supported | `UNVERIFIED` here; owned by the native CI matrix |

## Contract

### Authentication and Authorization

The file tools are canonical JARVIS tools: validated, policy-evaluated, approval-gated and audited like every
other. `files.list@1` and `files.read@1` are `ReadOnly`/`Low`; `files.write@1` is `Write`/`Moderate` and asks
unless the autonomy level or a standing approval covers it. A write is only **offered** when at least one
root is writable, and is refused `PermissionDenied` on a read-only root regardless of approval.

### Transport and Lifecycle

In-process, off the async threads (`spawn_blocking`). Roots are opened **once at startup** with the
operator's ambient authority (`Dir::open_ambient_dir`); afterwards the tools hold only the directory
capabilities. A root that cannot be opened, or a name used twice, fails startup (`StartupError::Config`).

### Data and Limits

Reads return at most 128 KiB (32 KiB by default) of UTF-8 text, cut on a character boundary; binary content
is refused `OutputInvalid`. Writes are bounded at 48 KiB, below the 64 KiB argument ceiling. A listing
returns at most 200 entries. A write never creates a directory, and uses `create_new` unless the model
passes `overwrite: true`. Results are labelled `Confidential`.

### Errors and Retries

`SchemaInvalid` (path shape), `PermissionDenied` (unknown root, read-only root), `NotFound`, `Conflict`
(file exists and no overwrite), `LimitExceeded`, `OutputInvalid`. `files.write@1` declares
`Idempotency::None`: a retry could replace a file, so the pipeline treats it as non-retryable.

## Security Analysis

- **Threat:** model-chosen paths. The model can name a root and a relative path and nothing else; roots are
  operator configuration, never model input.
- **Traversal:** refused by shape in the domain (`..`, absolute, drive, UNC, NUL, backslash) **and**
  structurally by `cap-std`, which resolves every component relative to the `Dir` handle.
- **Link escape:** a junction inside a root pointing outside it is not followed (verified on Windows).
  Symlinks are refused by the same mechanism per the README; unverified on this account.
- **TOCTOU:** the tools never check-then-act on a string path; every operation is performed through the
  handle, so a race cannot swap a validated name for another target.
- **Disclosure:** the approval prompt shows the root, path, and the first 160 characters of the content so
  consent is informed; control characters are stripped and the row count is bounded.
- **Residual risk:** `cap-std` is not a sandbox for untrusted code. A tool a model drives is not untrusted
  *code*, but a root should still not contain secrets the operator would not show the model: reads are
  `Low` risk and run unprompted at the default autonomy level.

## Normalization Map

| Provider concept | JARVIS concept |
| --- | --- |
| `cap_std::fs::Dir` | A named, operator-declared root |
| `io::ErrorKind::NotFound` | `ToolErrorClass::NotFound` |
| `io::ErrorKind::AlreadyExists` | `ToolErrorClass::Conflict` |
| Any other `io::Error` | `ToolErrorClass::ProviderError`, detail withheld |

## Falsifiable Claims

| ID | Claim | Falsified by |
| --- | --- | --- |
| FS-C001 | A path that escapes a root by shape is refused before the filesystem is touched | `a_path_that_leaves_the_root_by_shape_is_refused_before_the_filesystem_is_touched` |
| FS-C002 | A directory junction inside a root does not lead outside it | `a_directory_junction_that_points_outside_the_root_is_not_followed` |
| FS-C003 | A write to a read-only root is refused regardless of approval | `an_unknown_root_and_a_read_only_root_are_permission_failures` (fails when the writable check is removed) and `tests/e2e/files-journey.mjs` |
| FS-C004 | A write never overwrites unless `overwrite: true` | `a_write_refuses_to_replace_a_file_unless_asked` |
| FS-C005 | Reads and writes are bounded | `a_write_never_creates_directories_and_is_bounded`, `a_read_is_bounded_and_a_cut_inside_a_character_is_not_a_binary_file`, `a_listing_is_bounded_and_says_so` |
| FS-C006 | Nothing outside the root reaches the model | `files-journey.mjs` (sentinel file beside the root) |
| FS-C007 | With no roots no file tool is offered | `files-journey.mjs` "none" scenario |

## Test Plan

### Deterministic Tests

`crates/jarvis-infrastructure/src/native_tools/files_tests.rs` (13 tests; the symlink test skips itself without the privilege), the `[tools.files]` validation
tests in `config/loader.rs`, and the approval-row tests in `tool_adapters/preview_tests.rs`.

### Contract Fixtures

None: the dependency is a local library with no wire protocol.

### Gated Live Tests

`tests/e2e/files-journey.mjs` runs a real `jarvisd` over temporary directories with a scripted model. It is
not gated on a network or credential and runs in the normal e2e set.

## Operational Readiness

A root is described to the model in the tool's purpose, so the tool identity (and the action digest) stays
stable when roots change. Diagnostics name the failing class, never the path contents. Removing a root
requires a restart.

## Open Questions

- `UNVERIFIED`: symlink containment on Windows with the symlink privilege; run the skipped test on a
  privileged account or CI runner.
- `UNVERIFIED`: macOS and Linux builds and containment; owned by native CI.
- Per-root autonomy overrides (for example "always ask on this root") are not built.

## Change Log

- 2026-10-03: Initial evidence for `cap-std` 4.0.3 and the three file tools. New transitive packages
  (all permissively licensed, none yanked): `ambient-authority` 0.0.2, `cap-primitives` 4.0.3,
  `fs-set-times` 0.20.3, `io-extras` 0.19.0, `io-lifetimes` 3.0.1, `ipnet` 2.12.2, `linux-raw-sys` 0.12.1,
  `maybe-owned` 0.3.4, `rustix` 1.1.5, `rustix-linux-procfs` 0.1.1, `winx` 0.36.4. `cap-std` is
  `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`.
