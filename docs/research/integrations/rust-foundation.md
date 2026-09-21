# Integration Evidence: Rust Foundation

Status: ACCEPTED
Owner: Foundation
Last verified: 2026-09-21
Revalidate by: 2027-03-20
Implementation gate: PASSED

## Decision Summary

- Purpose: establish the exact toolchain, dependency, persistence, local HTTP,
  credential, filesystem, observability, and per-user service assumptions for
  Milestone 1.
- JARVIS boundary: these packages and OS facilities support the Rust control
  plane. They do not authorize model providers, connectors, MCP, desktop UI,
  external runtimes, voice providers, PostgreSQL, or public release services.
- Toolchain target: Rust and Cargo `1.98.1`, edition `2024`, Cargo resolver `3`.
- Storage target: SQLx `0.9.0` with `sqlite-bundled`; the resolved
  `libsqlite3-sys` must be `0.37.0`, which vendors SQLite `3.51.3`.
- Supported deployment modes: local per-user daemon and CLI on the target
  matrix below; portable foreground mode without service registration.
- Explicitly unsupported: network filesystems for mutable SQLite state,
  plaintext secret-store fallback, dynamic SQLite extension loading,
  elevated Windows services, OpenTelemetry export, public package publishing,
  production signing, and provider integrations.
- Kill switch or disable path: stop and disable/remove the per-user service, or
  run only in portable foreground mode. No Foundation component requires a
  hosted account, billing, or remote callback.

This note approves only the exact versions and feature sets named here. It is
not blanket approval for future dependencies, new features, version ranges, or
transitive changes. Every later manifest or lockfile change still travels with
this refreshed note or an eligible dependency-ledger row in the same changed-
path validator invocation.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| Rust release history | https://doc.rust-lang.org/1.98.1/releases.html | Rust 1.98.1, 2026-09-03 | 2026-09-21 | Stable patch and language/toolchain baseline |
| Rust target support | https://doc.rust-lang.org/1.98.1/rustc/platform-support.html | Rust 1.98.1 | 2026-09-21 | Target tiers and minimum OS/runtime constraints |
| Cargo workspaces | https://doc.rust-lang.org/cargo/reference/workspaces.html | Rust 1.98.1 docs | 2026-09-21 | Virtual workspace, resolver, inherited metadata and dependencies |
| Cargo manifests and lints | https://doc.rust-lang.org/cargo/reference/manifest.html | Rust 1.98.1 docs | 2026-09-21 | `rust-version`, edition, publish flag, and workspace lint inheritance |
| Standard file locking | https://doc.rust-lang.org/1.98.1/std/fs/struct.File.html#method.try_lock | Rust 1.98.1 | 2026-09-21 | Cross-platform exclusive lock behavior and `WouldBlock` |
| Standard rename | https://doc.rust-lang.org/1.98.1/std/fs/fn.rename.html | Rust 1.98.1 | 2026-09-21 | Same-filesystem replace behavior and platform differences |
| Tokio shutdown | https://tokio.rs/tokio/topics/shutdown | Tokio 1.x | 2026-09-21 | Detect, notify, and bounded-drain shutdown phases |
| Tokio cancellation | https://docs.rs/tokio-util/0.7.19/tokio_util/sync/struct.CancellationToken.html | tokio-util 0.7.19 | 2026-09-21 | Cooperative cancellation, cancel safety, and child-token behavior |
| Axum graceful shutdown | https://docs.rs/axum/0.8.9/axum/serve/struct.Serve.html#method.with_graceful_shutdown | Axum 0.8.9 | 2026-09-21 | Server shutdown signal and connection drain |
| Axum body limits | https://docs.rs/axum/0.8.9/axum/extract/struct.DefaultBodyLimit.html | Axum 0.8.9 | 2026-09-21 | Extractor limit scope and raw-body bypass |
| Tower HTTP body limit | https://docs.rs/tower-http/0.7.1/tower_http/limit/struct.RequestBodyLimitLayer.html | tower-http 0.7.1 | 2026-09-21 | Global request body enforcement |
| Tower middleware order | https://docs.rs/tower/0.5.3/tower/struct.ServiceBuilder.html#order | Tower 0.5.3 | 2026-09-21 | Layer order changes concurrency and buffering semantics |
| Serde container attributes | https://serde.rs/container-attrs.html | Serde 1.x | 2026-09-21 | `deny_unknown_fields` behavior and constraints |
| Jiff timestamp type | https://docs.rs/jiff/0.2.37/jiff/struct.Timestamp.html | jiff 0.2.37 | 2026-09-21 | RFC 3339 `Z` display, `FromStr` parsing, nanosecond precision, no leap seconds |
| Jiff serde helpers | https://docs.rs/jiff/0.2.37/jiff/fmt/serde/index.html | jiff 0.2.37 | 2026-09-21 | serde helpers are integer-based, so JARVIS uses string (de)serialization instead |
| UUID `now_v7` | https://docs.rs/uuid/1.26.1/uuid/struct.Uuid.html#method.now_v7 | uuid 1.26.1 | 2026-09-21 | Process-ordered v7 generation gated by the `v7` feature |
| Tokio-util sync module | https://docs.rs/tokio-util/0.7.19/tokio_util/sync/index.html | tokio-util 0.7.19 | 2026-09-21 | `CancellationToken` is not feature-gated on the `sync` module |
| serde_json deserializer | https://docs.rs/serde_json/1.0.151/serde_json/de/struct.Deserializer.html | serde_json 1.0.151 | 2026-09-21 | End-of-input and bounded recursion behavior |
| serde_json numbers | https://docs.rs/serde_json/1.0.151/serde_json/struct.Number.html | serde_json 1.0.151 | 2026-09-21 | Integer and floating-point representation limits |
| TOML parser | https://docs.rs/toml/1.1.6/toml/fn.from_str.html | toml 1.1.6+spec-1.1.0 | 2026-09-21 | Typed TOML document deserialization |
| TOML value feature | https://docs.rs/crate/toml/1.1.6+spec-1.1.0/features | toml 1.1.6+spec-1.1.0 | 2026-09-21 | `toml::Value` parsing requires the `unbounded` feature; JARVIS avoids it |
| Clap parser | https://docs.rs/clap/4.6.7/clap/trait.Parser.html | clap 4.6.7 | 2026-09-21 | Fallible CLI parsing without process exit |
| SQLx SQLite options | https://docs.rs/sqlx/0.9.0/sqlx/sqlite/struct.SqliteConnectOptions.html | SQLx 0.9.0 | 2026-09-21 | Connection defaults, pragmas, buffers, and extension risk |
| SQLx migrator | https://docs.rs/sqlx/0.9.0/sqlx/migrate/struct.Migrator.html | SQLx 0.9.0 | 2026-09-21 | Migration locking and applied-checksum validation |
| SQLx embedded migrations | https://docs.rs/sqlx/0.9.0/sqlx/macro.migrate.html | SQLx 0.9.0 | 2026-09-21 | Stable build-script and LF hash requirements |
| SQLx SQLite dependency source | https://crates.io/crates/sqlx-sqlite/0.9.0 | SQLx 0.9.0 | 2026-09-21 | `libsqlite3-sys >=0.30.1,<0.38.0` and feature mapping |
| Bundled SQLite source | https://crates.io/crates/libsqlite3-sys/0.37.0 | libsqlite3-sys 0.37.0 | 2026-09-21 | Vendored `SQLITE_VERSION` is `3.51.3` |
| SQLite WAL | https://sqlite.org/wal.html | Updated 2026-08-25 | 2026-09-21 | Concurrency, checkpointing, persistent files, and WAL-reset fix |
| SQLite transactions | https://sqlite.org/lang_transaction.html | Updated 2026-02-18 | 2026-09-21 | One-writer behavior, `BEGIN IMMEDIATE`, BUSY, and rollback state |
| SQLite pragmas | https://sqlite.org/pragma.html | Updated 2026-06-04 | 2026-09-21 | Durability, integrity, trusted schema, and pragma caveats |
| SQLite backup | https://sqlite.org/backup.html | Updated 2025-11-13 | 2026-09-21 | Consistent online snapshots and incremental busy handling |
| SQLite release history | https://sqlite.org/changes.html | SQLite 3.53.4, 2026-07-24 | 2026-09-21 | Current release and 3.51.3 WAL-reset repair threshold |
| Platform directories | https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html | directories 6.0.0 | 2026-09-21 | XDG, Known Folder, and macOS standard path mappings and `runtime_dir`/`state_dir` nullability |
| Keyring v1 adapter | https://docs.rs/keyring/4.2.0/keyring/v1/index.html | keyring 4.2.0 | 2026-09-21 | Keychain, Credential Manager, and Secret Service backends |
| Apple Keychain Services | https://developer.apple.com/documentation/security/keychain-services | Current 2026 docs | 2026-09-21 | Encrypted keychain storage for small user secrets |
| Windows Credential Manager | https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew | Updated 2024-11-20 | 2026-09-21 | Current-logon credential set and explicit failure codes |
| Secret Service specification | https://specifications.freedesktop.org/secret-service/latest/ | 0.2 DRAFT, 2026-04-08 | 2026-09-21 | Session, collection, lock, prompt, and D-Bus error model |
| Systemd unit source | https://raw.githubusercontent.com/systemd/systemd/main/man/systemd.unit.xml | Upstream main | 2026-09-21 | User unit paths, enablement, and start-rate limits |
| Systemd service source | https://raw.githubusercontent.com/systemd/systemd/main/man/systemd.service.xml | Upstream main | 2026-09-21 | Restart, startup, stop timeout, and process termination |
| Apple LaunchAgent guide | https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html | Apple archive | 2026-09-21 | Per-user launch agent ownership and lifecycle model |
| Windows Task Scheduler | https://learn.microsoft.com/en-us/windows/win32/taskschd/task-scheduler-start-page | Task Scheduler 2.0 | 2026-09-21 | Logon triggers and supported Windows versions |
| Windows task security | https://learn.microsoft.com/en-us/windows/win32/taskschd/security-contexts-for-running-tasks | Updated 2025-05-12 | 2026-09-21 | Low-privilege current-user interactive-token tasks |
| Windows atomic replacement | https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew | Updated 2025-07-01 | 2026-09-21 | Same-volume replacement, ACL preservation, and partial failure states |
| Tracing formatter | https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/fmt/index.html | tracing-subscriber 0.3.23 | 2026-09-21 | JSON lines, filters, and custom field formatting |
| Tracing appender | https://docs.rs/tracing-appender/0.2.4/tracing_appender/non_blocking/index.html | tracing-appender 0.2.4 | 2026-09-21 | Bounded queue, dropped-line counter, and shutdown guard |
| OS randomness | https://docs.rs/getrandom/0.4.1/getrandom/ | getrandom 0.4.1 | 2026-09-21 | `fill(&mut buf)` returns `Result<(), Error>`; failure is never weak bytes |
| OS randomness | https://docs.rs/getrandom/0.4.1/getrandom/ | getrandom 0.4.1 | 2026-09-21 | Native secure RNG sources and fail-closed errors |
| SHA-256 | https://docs.rs/sha2/0.10.9/sha2/ | sha2 0.10.9 | 2026-09-21 | SHA-2 implementation and digest API |
| Constant-time equality | https://docs.rs/subtle/2.6.1/subtle/trait.ConstantTimeEq.html | subtle 2.6.1 | 2026-09-21 | Fixed-shape digest comparison |
| Secret wrapper | https://docs.rs/secrecy/0.10.3/secrecy/ | secrecy 0.10.3 | 2026-09-21 | Explicit exposure, non-serializing default, and zeroize-on-drop |

Attempted `llms.txt` URLs that did not exist or did not provide a usable index:

- `https://doc.rust-lang.org/llms.txt`
- `https://www.rust-lang.org/llms.txt`
- `https://tokio.rs/llms.txt`
- `https://docs.rs/llms.txt`
- `https://docs.rs/axum/latest/llms.txt`
- `https://docs.rs/sqlx/latest/llms.txt`
- `https://sqlite.org/llms.txt`
- `https://docs.rs/keyring/latest/llms.txt`
- `https://jiff.rs/llms.txt`
- `https://docs.rs/jiff/latest/llms.txt`

The official documentation navigation, versioned docs.rs source, official
repositories, and registry metadata were used after those attempts failed.

## Version and License Matrix

### Toolchain and targets

| Component | Exact target | License or support state | Decision |
| --- | --- | --- | --- |
| Rust/Cargo | 1.98.1 | Rust distribution components carry their upstream licenses | Pin with `rust-toolchain.toml`; do not infer a JARVIS project license |
| `x86_64-pc-windows-msvc` | Windows 10+ | Rust Tier 1 with host tools | Supported and required in native CI |
| `aarch64-apple-darwin` | macOS 11+ | Rust Tier 1 with host tools | Supported and required in native CI |
| `x86_64-apple-darwin` | macOS product target | Rust Tier 2 with host tools since Rust 1.90 | Supported by JARVIS only with native CI and packaged proof |
| `x86_64-unknown-linux-gnu` | kernel 3.2+, glibc 2.17+ | Rust Tier 1 with host tools | Supported and required in native CI |
| `aarch64-unknown-linux-gnu` | kernel 4.1+, glibc 2.17+ | Rust Tier 1 with host tools | Supported and required in native CI |

Tier status is not release proof. Native clean-machine service, path,
permission, package, and crash tests remain mandatory for every JARVIS target.

**Bundled SQLite requires a native C toolchain per target.** `sqlite-bundled`
compiles the vendored `sqlite3.c` through `cc`, so a build for a non-host target
needs that target's C compiler and linker. Verified locally: `cargo check
--target x86_64-unknown-linux-gnu` from Windows fails with
tool "x86_64-linux-gnu-gcc" not found`. Cross-compilation from one host is
therefore **not supported** for the workspace once storage is included, and each
target's compile-and-test proof must come from native CI. The earlier FND-001
statement that five targets compile from one host applies to the
pre-storage workspace only and is superseded here.

### Approved Foundation packages

| Package | Exact version | Enabled features or use | License | Boundary decision |
| --- | --- | --- | --- | --- |
| tokio | 1.53.1 | `fs`, `io-util`, `macros`, `net`, `process`, `rt-multi-thread`, `signal`, `sync`, `time` | MIT | Runtime and bounded lifecycle only |
| tokio-util | 0.7.19 | `rt` | MIT | Cooperative `CancellationToken` |
| axum | 0.8.9 | `http1`, `json`, `tokio` | MIT | Loopback local API only |
| tower | 0.5.3 | `limit`, `timeout`, `util` | MIT | Outer total concurrency and timeouts |
| tower-http | 0.7.1 | `limit`, `request-id`, `trace` | MIT | Global 64 KiB body cap and correlation |
| serde | 1.0.229 | `derive` | MIT OR Apache-2.0 | Explicit versioned DTOs |
| serde_json | 1.0.151 | defaults; no `arbitrary_precision` or `unbounded_depth` | MIT OR Apache-2.0 | Bounded JSON contracts |
| sqlx | 0.9.0 | `runtime-tokio`, `sqlite-bundled`, `migrate`, `macros`; no broad `sqlite` feature | MIT OR Apache-2.0 | SQLite adapter and embedded migrations |
| libsqlite3-sys | 0.37.0, transitive lock pin | `bundled` through SQLx | MIT; vendored SQLite public domain | Must resolve SQLite 3.51.3 |
| tracing | 0.1.44 | `attributes` | MIT | Structured events only |
| tracing-subscriber | 0.3.23 | `env-filter`, `fmt`, `json`, `registry` | MIT | Local JSON lines and runtime filtering |
| tracing-appender | 0.2.4 | default | MIT | Bounded off-thread writer and fixed-time rotation |
| clap | 4.6.7 | `derive` | MIT OR Apache-2.0 | Typed CLI contract |
| uuid | 1.26.1 | `serde`, `v7` | Apache-2.0 OR MIT | Typed, process-ordered identifiers |
| jiff | 0.2.37 | `default-features = false`, `std`, `serde` | Unlicense OR MIT | RFC 3339 UTC instants; no time zone database |
| thiserror | 2.0.20 | default | MIT OR Apache-2.0 | Explicit library-boundary errors |
| directories | 6.0.0 | default | MIT OR Apache-2.0 | Standard per-user paths |
| keyring | 4.2.0 | `default-features = false`, `v1` | MIT OR Apache-2.0 | Native secret stores; no plaintext fallback |
| toml | 1.1.6+spec-1.1.0 | `parse`, `serde`, `display` | MIT OR Apache-2.0 | Versioned config document |
| getrandom | 0.4.1 | default native backends | MIT OR Apache-2.0 | 256-bit local bearer credentials |
| sha2 | 0.10.9 | default | MIT OR Apache-2.0 | Stored SHA-256 credential verifier |
| subtle | 2.6.1 | default | BSD-3-Clause | Constant-time digest equality |
| base64 | 0.22.1 | `alloc`, `std` | MIT OR Apache-2.0 | Unpadded base64url credential presentation |
| secrecy | 0.10.3 | no serde serialization | Apache-2.0 OR MIT | Explicit in-memory secret exposure |
| tempfile | 3.23.0 | test-only | MIT OR Apache-2.0 | Isolated filesystem and SQLite tests |

No package is published: every workspace package sets `publish = false` while
`OWN-001` remains blocked. The lockfile, enabled feature graph, crate license
files, advisories, duplicate native libraries, and bundled C source are checked
again before distribution. SQLite's public-domain dedication does not select a
license for JARVIS.

`fs4` is rejected: Rust 1.98.1 includes stable `File::lock`, `try_lock`, and
`unlock`. OpenTelemetry packages are not selected for Foundation; local
structured logs are the only telemetry sink in this gate.

`jiff` is pinned with `default-features = false` because its default set also
enables `tz-system`, `tz-fat`, `tzdb-bundle-platform`, `tzdb-zoneinfo`, and
`tzdb-concatenated`. JARVIS stores absolute instants as RFC 3339 UTC and has no
Foundation requirement for a time zone database, so those features and their
platform database plumbing are deliberately omitted. Its dual
`Unlicense OR MIT` license is compatible with either JARVIS license candidate and
does not select one.

## Foundation Contract

### Runtime, Cancellation, and Shutdown

- Cancellation is cooperative. `CancellationToken::cancel()` propagation is
  not treated as an atomic observation across child tokens while cancellation
  is in progress; after it returns, descendants are cancelled.
- A child token cannot cancel its parent. Every spawned task receives the
  correct scoped token and correlation context.
- Shutdown detects Ctrl-C, SIGTERM where available, or an authenticated daemon
  request; stops admission; cancels task roots; waits a configured bounded
  drain; persists terminal state; flushes logs; then exits.
- `run_until_cancelled()` does not make a non-cancel-safe future safe and favors
  future completion in a simultaneous race. Side effects therefore use durable
  intent/idempotency records rather than relying on future cancellation.
- Process, channel, body, row, queue, and captured-output bounds are explicit.

### Local HTTP Boundary

- Bind to parsed loopback addresses only. Remote binding is outside this gate.
- The v0.1 request contract is 64 KiB. `RequestBodyLimitLayer` is global because
  Axum's default extractor limit does not constrain raw body consumption.
- An outer concurrency limit counts active and queued work. Buffers are not
  placed outside that limit because doing so increases total admitted work.
- Per-route schema validation remains required after the byte limit.
- Graceful shutdown stops new admission and drains connections within a bound;
  a runtime claim of graceful drain is tested with real in-flight requests.

### Serialization and Configuration

- Every public or persisted object has an explicit schema/version field and
  serialization tests. Unknown versions fail closed.
- Security-sensitive structs use `deny_unknown_fields`; forward-compatible
  event payloads may preserve an explicit extensions map instead of silently
  accepting arbitrary fields.
- JSON input is byte-bounded before parsing, checks deserializer end-of-input,
  and retains serde_json's recursion limit. `unbounded_depth` is forbidden.
- JSON contract integers stay within signed/unsigned 64-bit ranges unless a
  field explicitly uses a validated decimal string. NaN and infinities are not
  JSON numbers.
- TOML files are typed, versioned, size-bounded, and reject duplicate or unknown
  contract keys through the parser and target type. Environment overrides are
  an explicit allowlist; arbitrary environment-to-key projection is forbidden.
- CLI library paths use `try_parse*` so callers receive typed errors rather than
  an unexpected process exit. Secret values are never accepted in command-line
  arguments because process listings and shell history can expose them.
- Config writes create an owner-only temporary file in the destination
  directory, write and sync it, replace the destination, then verify the
  resulting schema and permissions. A failure preserves the prior readable
  config and reports any ambiguous Windows `ReplaceFileW` state for repair.

### Configuration Layering and Secrets

- Precedence is built-in defaults, then the config file, then the environment
  allowlist, then command-line overrides. Command line is highest, matching
  operator expectation; a test asserts the full order rather than one pair.
- Only `JARVIS_LOG_LEVEL`, `JARVIS_STORAGE_KIND`, and `JARVIS_MODEL_POLICY_ID`
  may be overridden by the environment. Any other `JARVIS_`-prefixed variable
  is reported as ignored, which makes the forbidden arbitrary
  environment-to-key projection visible instead of silent.
- Configuration is non-secret by construction. A file carries a secret
  *reference* such as `env:JARVIS_MODEL_KEY`; the value is resolved at the last
  responsible moment through a `SecretResolver` port. An operator who pastes a
  raw value where a reference belongs gets a parse failure, and neither the
  config `Debug`, the rendered TOML, nor the error message contains the value.
- The environment provider only accepts locators prefixed `JARVIS_`, so a
  configuration file cannot direct JARVIS to read an unrelated ambient secret
  such as a cloud provider key or a CI token.
- `schema_version` is read through a small typed probe before the full parse, so
  an unsupported version is reported as `jarvis.config_version_unsupported`
  independently of field problems, and an unsupported document is never
  rewritten.
- Every level uses `deny_unknown_fields`, so a typo fails loudly rather than
  being ignored. `toml::Value` is deliberately not used because it requires the
  `unbounded` feature, which conflicts with the bounded-input requirement.
- Writes are atomic: a fresh owner-only temp file in the destination directory,
  write, `sync_all`, `rename`, then a directory flush. The temp file is created
  with `create_new`, so a symlink planted at the temp name cannot redirect the
  write. A failure leaves the previous file intact and removes the temp file.

### Time, Identifiers, and Errors

- Canonical identifiers are typed newtypes over `uuid` 1.26.1 `Uuid`.
  `now_v7()` generation requires the `v7` feature and stays behind a
  `jarvis-domain` `IdGenerator` port so a deterministic implementation can be
  injected in tests. A `Uuid` value is canonical by construction, so every
  presentation is lowercase hyphenated and callers cannot build an uppercase or
  non-hyphenated form. Identifiers remain opaque and case-sensitive.
- `Display` and `Debug` render the inner UUID with `{}`, which is lossless and
  round-trips. `Serialize` emits the canonical string and `Deserialize` uses
  `Uuid::try_parse`, so braces, an urn prefix, uppercase hex, or trailing data
  are rejected rather than normalized. A deserialized ID is never accepted as
  authorization context.
- Absolute time is a `jarvis-domain` newtype over `jiff::Timestamp`. `Display`
  emits RFC 3339 UTC with `Z` (`2005-08-07T23:19:49.123Z`). Parsing accepts RFC
  3339/ISO 8601/Temporal input and normalizes it, so `2024-06-19 15:22:45-04`
  parses and displays as `2024-06-19T19:22:45Z`. `Timestamp` is
  nanosecond-precision and cannot represent leap seconds: a parsed second of
  `60` is constrained to `59`.
- `Timestamp::now()` panics on an unreasonable system clock and is not used in
  library code. `jarvis-infrastructure` provides a `SystemClock` that reads the
  OS clock fallibly (`SystemTime` plus `TryFrom<SystemTime>`) and surfaces the
  domain error instead of panicking.
- Domain library errors are `thiserror` 2.0.20 enums. Each variant carries a
  stable namespaced code (for example `jarvis.invalid_timestamp`) and a
  `retryable` flag describing this operation, not the error class in every
  context. Library errors stay typed; context is added at application boundaries.
- Cancellation, deadlines, and correlation live in `jarvis-application`, not the
  domain. `RequestContext` is server-derived and is not assembled from body
  fields. `jarvis-domain` deliberately does not depend on `tokio-util`.

### Windows Permission Enforcement Decision

- Windows Known Folders are ACL-protected by the OS. Local inspection of
  `%LOCALAPPDATA%` (`icacls`) shows only the owner, `SYSTEM`, and Administrators,
  all inherited. JARVIS mutable data therefore starts owner-restricted without
  any JARVIS code writing a discretionary ACL.
- Writing or querying an explicit Windows DACL requires `unsafe` access to
  `AddAccessAllowedAceEx`, `GetNamedSecurityInfo`, or an equivalent. Every
  library crate carries `#![forbid(unsafe_code)]`, and the dependency ledger
  marks ACL-affecting crates as non-routine.
- Decision for `FND-003`: enforce and prove permissions where the standard
  library can do so safely, and enforce Windows containment instead of an ACL:
  - Unix: directories are created with `DirBuilderExt::mode(0o700)` and files
    with `OpenOptionsExt::mode(0o600)`. The process `umask` can only *clear*
    bits, so these are upper bounds and the result can never be more permissive
    than requested. `umask` is never modified, because it is process-global and
    racing it from a multi-threaded daemon is unsafe. The resulting mode is
    queried back, and a directory with any group/other bit set is reported as
    unsafe rather than silently used.
  - Windows: every resolved mutable path is proven to live under a Known Folder
    (`FOLDERID_LocalAppData` or `FOLDERID_RoamingAppData`), and path components
    are rejected when they are absolute, rooted, or contain `..`. This is the
    safe, `unsafe`-free part of the containment requirement.
- Explicit Windows DACL write and query-back remains **DEFERRED** with an owner.
  It requires either a narrowly scoped `unsafe` binding that overrides the crate
  guard by approved ADR, or a reviewed safe wrapper crate. The deferral is
  falsifiable: `ACC-006` requires owner-only files to reject unsafe permissions,
  and that assertion is executed on Unix now and recorded as a Windows stop
  condition until the ADR lands. No JARVIS code claims Windows ACL enforcement
  it does not perform.
- On Windows, `ProjectDirs::runtime_dir()` and `state_dir()` return `None`.
  JARVIS therefore derives the local runtime/state root from
  `ProjectDirs::data_local_dir()` on those platforms instead of treating a
  missing XDG-style directory as an error.

### Paths, Permissions, and Locks

- `ProjectDirs::from("com", "JARVIS", "JARVIS")` is the canonical standard-
  path source. Failure to resolve a home/profile is explicit, never replaced by
  the current directory.
- Mutable database/runtime/log data uses local, non-roaming paths on Windows.
  User-edited config may use the platform config path. Tests assert the exact
  resolved table on each native OS.
- Unix directories are created owner-only and files are created with mode
  `0o600`, accounting for `umask`; permissions are queried back. Windows uses
  an owner/system-only DACL and queries it back before publishing discovery or
  credential material. The exact Windows binding must be reviewed with the
  manifest change that introduces it.
- Single-instance ownership uses a held writable file handle and
  `try_lock()`. A lock file's contents are diagnostic only; PID text never
  proves ownership. A stale unlocked file is repairable.
- Re-locking the same handle or a clone is forbidden because behavior is
  platform dependent. Drop releases the lock; explicit unlock errors are
  surfaced during orderly shutdown when relevant.
- Symlinks/reparse points and path canonicalization are checked before
  authorization-sensitive access. Portable mode keeps all state under its
  explicit root and does not register a service.

### Credentials and Native Secret Stores

- A local bearer credential is exactly 32 bytes generated by `getrandom`; RNG
  failure aborts enrollment/startup rather than returning weak bytes.
- The presented token is unpadded base64url. Only a SHA-256 verifier is stored
  in normal daemon records, and equal-length 32-byte digests are compared with
  `subtle::ConstantTimeEq`.
- SHA-256 is suitable here because the input has 256 bits of OS-generated
  entropy. Human-memorable passwords require a separately researched password
  hashing design and are outside this contract.
- The client copy is *intended* to be stored through keyring 4.2.0: macOS Keychain
  Services, Windows Credential Manager, or freedesktop Secret Service. Store
  unavailable, locked, prompting, denied, and missing-item states are distinct
  diagnostics. **No keyring adapter exists yet**, so this is not the current
  behaviour; see the deviation recorded below, which is explicit rather than a
  silent fallback.
- Once the keyring adapter lands, a Linux headless session with no Secret Service
  or a locked collection fails closed and offers an explicit
  re-enrollment/operator path; it never writes the token to a plaintext fallback.
  Until then, that statement is a *target*, not a description.
- **Current deviation, recorded rather than implied.** `FND-007` stores the client
  copy as an owner-only file in the profile config directory
  (`client-credential`, mode `0o600` on Unix), and the operator is told. This is a
  temporary, visible exception to "store in the OS credential store where
  available" and it is *not* the silent plaintext fallback forbidden above for
  two reasons: it is never entered as a fallback after a store failure (no store
  code runs at all), and it is reported in operator documentation. `RF-C010` is
  therefore **PARTIAL** until the keyring slice completes; the owning TODO is
  `FND-008` and the keyring adapter. Anyone auditing secret handling should treat
  this file as the current at-rest secret and not assume keychain storage.
- Secret wrappers prevent ordinary `Debug`/serialization exposure and zeroize
  owned storage on drop. They do not guarantee removal of earlier copies,
  registers, swap, crash dumps, or microarchitectural leakage, so code avoids
  cloning, reallocating, formatting, and logging secrets.

### SQLite and Migrations

- Only SQLx `sqlite-bundled` is enabled. SQLx's broad `sqlite` feature is
  forbidden because it also enables dynamic extension loading and other
  capabilities not needed by JARVIS. Verified from the SQLx 0.9.0 manifest:
  `sqlite = [sqlite-bundled, sqlite-deserialize, sqlite-load-extension,
  sqlite-unlock-notify]`, whereas `sqlite-bundled` pulls only
  `sqlx-sqlite/bundled`. Selecting the bundle is what keeps
  `sqlite-load-extension` (and with it `sqlite3_load_extension`) out of the graph.
- A connected handle is checked at runtime with `SELECT sqlite_version()` and must
  report at least `3.51.3`. The build-time pin is not sufficient proof: the
  runtime string is the one the database actually executes, and a future lockfile
  change cannot silently reintroduce a WAL-reset-vulnerable version.
- The check is an **at-least floor**, not an equality test. The pinned lockfile
  currently resolves `3.51.3`, but pinning equality would turn a future security
  patch (which is always at or above `3.51.3`) into a startup failure. The
  requirement is "not below the WAL-reset repair", so `>=` is the correct
  predicate and `cargo tree -i libsqlite3-sys` proves the native source.
- Mutable state must be on a local filesystem. WAL does not support a network
  filesystem and has one writer even though readers and a writer can overlap.
- JARVIS explicitly requests and queries back: foreign keys ON, WAL, synchronous
  FULL, trusted schema OFF, normal locking, bounded busy timeout, bounded SQLx
  command/row buffers, and a finite WAL/checkpoint policy. Unknown pragmas are
  silently ignored by SQLite, so setting without readback is insufficient.
- SQL statement logging is disabled because statements and bound context can
  contain sensitive data. Application-level events use allowlisted fields.
- Long readers can starve checkpoints and grow the WAL. Health reports WAL size
  and checkpoint progress; maintenance uses bounded checkpoint attempts and
  never deletes `-wal` or `-shm` files directly.
- `BEGIN IMMEDIATE` reserves write intent and may return BUSY. A BUSY COMMIT can
  leave the transaction active and retryable. FULL, IO, interrupt, and memory
  errors may roll back one statement or the whole transaction; repository code
  normalizes state before retry/return.
- SQLx migration locking remains enabled, applied checksums are validated,
  missing applied migrations are not ignored, and the migration table is not
  renamed. Stable embedding uses a build script with
  `cargo:rerun-if-changed=migrations`.
- Migration SQL is normalized with `*.sql text eol=lf` because SQLx migration
  hashes are line-ending-sensitive.
- Backups use SQLite's Online Backup API or a separately proven `VACUUM INTO`
  path, never a live copy of only the main file. Restore verifies
  `integrity_check`, `foreign_key_check`, schema version, and application
  postconditions before activation.

### Local Transport and Daemon Lifecycle

- The discovery file is written to the runtime directory with the atomic,
  owner-only path already implemented in `config::atomic`: a fresh `create_new`
  temp file, write, `sync_all`, `rename`, then a directory flush. `instance_id`
  is generated before the write, so a partially written file cannot be observed.
- Startup order is fixed and each step fails closed: acquire the single-instance
  lock, open and version-check the database, apply or refuse migrations, bind
  loopback, publish discovery, then mark ready. Readiness is a separate flag from
  process liveness, so `/health/live` succeeds while `/health/ready` returns 503
  during startup, drain, or failed migration.
- Loopback binding only. The listener binds a parsed `127.0.0.1` address, never a
  wildcard, and rejects an `Origin` header because no CORS or browser origin is
  trusted in local mode. `Host` is validated against the bound numeric authority.
- The local bearer credential is 32 bytes from `getrandom::fill`. The presented
  form is unpadded base64url, and only a SHA-256 verifier is stored. Comparison
  uses `subtle::ConstantTimeEq` on equal-length digests. A failure to obtain
  entropy aborts enrollment rather than returning weak bytes.
- Because Foundation has no credential-store adapter yet, the client copy is
  written to an owner-only file in the profile config directory and the operator
  is warned. This is a **documented, temporary** deviation from "store in the OS
  credential store where available": it is never a silent fallback, it uses the
  same owner-only permission path that is already proved on Unix, and `FND-008`
  plus the keyring slice replace it. Treating the OS store as unavailable-but-
  still-fine would be the real failure, so it is called out rather than implied.
- Drain is bounded: stop admission, cancel task roots, wait a configured grace
  period, persist terminal state, flush logs, then exit. A timeout does not block
  exit indefinitely; the incomplete drain is recorded and reconciled on restart.
- The service-manager facilities (systemd/launchd/Task Scheduler) are **not** in
  this slice; they are `FND-009`. `FND-007` proves foreground and portable
  lifecycle only.

- The CLI never opens the database for normal product commands. It discovers the
  daemon through the published file and calls the authenticated API, so the
  daemon stays the single authority. Argument parsing uses `try_parse` so a
  library path cannot exit the process, and no secret is accepted as a command-
  line argument.
- The Foundation client is a minimal HTTP/1.1 exchange over a numeric loopback
  peer rather than a general HTTP client. The daemon is one local peer, so a full
  client stack would add dependencies without adding capability, and the hand-rolled
  path makes it structurally hard to send the credential anywhere but the
  discovered loopback authority (the host is re-validated at call time even
  though the discovery file was validated at parse time).
- CLI responses are parsed into the contract shape rather than echoed. An
  unexpected body is rejected, so a compromised or mismatched daemon cannot get
  arbitrary text onto the operator's terminal.
- Exit codes are meaningful: `0` for success, `1` when the operator must act.
  Doctor reports blocking findings separately from warnings, so "no blocking
  findings" and "nothing is wrong" are not confused.
- Closing the FND-007 gap: the serve loop is now bound. `RunningDaemon::serve_until`
  drives `axum::serve` with a graceful shutdown future, then drains. The daemon
  therefore serves only while that future is driven, which is the shape a real
  process has; a test that queries the port before driving it would hang, and
  that is documented in the end-to-end test rather than left as a trap.

### Observability

- Redaction is applied by the **writer**, not the formatter. Every byte of every
  formatted record flows through the writer, so switching `.json()` for
  `.compact()` or adding a layer cannot silently bypass redaction. The writer
  buffers until a newline, redacts, then emits, and it also redacts the trailing
  fragment on `flush`.
- Redaction has two layers with different strength, and the difference is
  recorded rather than implied: registered-value replacement is the guarantee
  (proved by the canary test), while pattern replacement for authorization
  headers, `key=value` credential pairs, and URL userinfo is best-effort defense
  in depth. A value shorter than 8 bytes is refused at registration rather than
  redacted, because redacting it would corrupt unrelated output.
- `tracing-appender` is pinned exactly (`=0.2.4`). A `^0.2.4` requirement resolved
  to `0.2.5`, a patch release this note did not review and whose source the API
  was not verified against. An exact pin keeps the reviewed version in the
  lockfile; a future upgrade is a new review.
- The non-blocking queue is bounded and set to `lossy(false)` so backpressure is
  preferred to dropping a line that may carry audit context. The queue is still
  bounded, so a stalled disk cannot grow memory without limit. The dropped-line
  counter is retained and exposed so degraded observability is visible.
- Rotated files are created on schedule but are not deleted by the library, so
  `max_log_files` is what bounds total disk use. JARVIS-owned age/byte cleanup
  and console-format parity remain for a later slice; this slice installs the
  file sink only.
- **Named gaps against the observability architecture, not silently deferred.**
  The architecture requires rotation "by size/time" and disk-pressure behavior
  ([observability.md](../../architecture/observability.md)): `FND-005` implements
  time-based rotation with a retained-file bound but **not** size-based rotation,
  and has **no** disk-pressure behavior (a full disk yields an I/O error on the
  writer, not a bounded degraded mode). Console-format parity and the separate
  debug/audit/diagnostic log semantics are also not implemented. These are
  stop conditions for the daemon slice (`FND-007`) and diagnostics (`FND-013`),
  which must either satisfy them or record why not.
- The `WorkerGuard` is retained for daemon lifetime and dropped only after task
  drain, which is what flushes buffered lines on orderly shutdown.
- A seeded canary test scans the file sink. Console logs, errors, diagnostics,
  and support bundles are additional sinks that later slices must cover; no sink
  is considered covered merely because another sink is safe.

### Per-User Service Facilities

- Linux installs a user unit under the XDG/systemd user configuration path and
  controls it with `systemctl --user`. Enablement and starting are separate;
  install uses enable plus explicit start, and removal stops, disables, removes,
  reloads, then verifies absence. The service uses bounded stop time and
  restart-on-failure with start-rate limiting. A missing user manager/session is
  an actionable unsupported runtime state, not an elevation prompt.
- macOS installs a user-owned LaunchAgent under `~/Library/LaunchAgents` and
  manages the `gui/<uid>` domain. It never installs a root LaunchDaemon. Native
  tests must prove bootstrap, crash restart/throttling, bootout, malformed plist,
  and login/logout behavior on every supported macOS architecture.
- Windows uses Task Scheduler 2.0 with a current-user logon trigger,
  `TASK_LOGON_INTERACTIVE_TOKEN`, and low (`LUA`) run level. It does not store a
  password and does not use LocalSystem, an administrator group, or the Service
  Control Manager. The creating user owns read/update/delete/run rights by
  default. It is intentionally available only in an interactive user session.
- Service definitions contain executable/config paths and non-secret switches
  only. Provider keys and local bearer tokens are never embedded in unit files,
  plists, task XML, command lines, or service-manager environment blocks.
- Service install/remove is idempotent and has preview, verification, and repair
  paths. Portable mode is the universal kill switch when a facility is absent.

#### FND-009 implementation state

The design deliberately separates **plan** from **execute**. A `ServicePlan`
names the facility, the exact program, the exact argument vector, the exact
definition content, and the exact path written, and it renders as an operator
preview. Nothing is executed by planning, so every platform's plan is asserted on
every host, including the host running this work.

What is implemented and verified:

- Three controllers behind one `ServiceController` trait: `SystemdController`,
  `LaunchdController`, and `WindowsTaskController`.
- Argument vectors are asserted exactly per backend, including the absence of
  elevation, and the absence of a credential on Windows. These tests execute on
  any host because they assert a plan rather than run a service manager.
- Definitions are rendered with the escaping their format requires rather than
  relying on validation alone: `systemd` values are quoted and their significant
  characters escaped, and XML text nodes are entity-escaped. Validation of
  control characters is a second, independent barrier.
- The Windows read-only status query was verified against the real `schtasks` on
  this host: querying an absent task exits non-zero, which maps to
  `NotInstalled` rather than to an error.
- The end-to-end report path was verified live: `jarvis service` on Windows
  printed `backend: windows-scheduled-task`, `service: not_installed`, and the
  exact `/create` argument vector naming the resolved `jarvisd` executable.

What is not yet verified, and is therefore not claimed:

- No service was installed, started, stopped, or removed on any platform. Actual
  registration, crash restart, throttling, logout/logon persistence, and removal
  are **native-CI work assigned to `FND-010`**.
- The `systemd` and `launchd` controllers are asserted as plans only; no
  `systemctl` or `launchctl` invocation has been observed.
- `ExecStop`/`Bootout` ordering during a drain is not asserted; the bound
  (`TimeoutStopSec=15`) is asserted in the definition only.
- `FND-009` is therefore `PARTIAL` until the native proofs run.

One defect was found by this slice's own tests and is recorded because it is the
kind a fixture cannot catch: the `LaunchAgent` `Label` was generated from the bare
service name while the plist filename and the `launchctl bootstrap` target used
the namespaced `com.jarvis.<name>`. launchd matches a service to its definition by
the `Label`, so the mismatch would have made the agent impossible to load or
remove. The test that caught it compares the rendered `Label` against the filename
and against the bootstrap target.

### Profile resolution (added by `FND-010`)

`jarvis_infrastructure::profile::resolve` is now the single decision point for the
profile, and both binaries use it. Precedence is explicit: an operator-supplied
`--profile <DIR>` root wins, otherwise the standard per-user profile.

Two things follow that matter for security:

1. **There is deliberately no environment override.** An environment variable that
   redirects the profile would move durable state, the client credential, and the
   discovery file for an installed product. On Windows the platform directory
   provider ignores `APPDATA`/`LOCALAPPDATA` overrides anyway, so such a variable
   would exist purely as attack surface with no portability benefit on that
   platform.
2. **The daemon matches the client.** Both take the same `--profile` option and
   call the same resolver, so a clean-machine run cannot have the two disagree
   about which profile they are using. The daemon records which source won.

`--profile` is what makes the clean-machine lane possible: it is an operator
decision the process can see, rather than ambient environment state that a test
must set and clean up.

## CI Gates

`FND-010` added two workflows and a clean-machine journey. The full evidence —
researched runner labels, verified action versions, the mutable-tag caveat, and the
falsifiable claims — is in
[the GitHub Actions evidence note](github-actions.md). The operator-facing
description, including what CI does **not** prove, is in
[CI gates and native lanes](../../operations/ci-gates.md).

The parts that belong in this note, because they concern JARVIS's own behavior:

- The clean-machine journey asserts **profile contents**, not only exit codes. An
  exit code of 0 with nothing written would still be a failure.
- The journey terminates the daemon in a `finally` block, so a failed assertion
  cannot leak a process into a later step. A leaked daemon holding the profile lock
  would make a later, unrelated step fail misleadingly.
- The journey deliberately does **not** register a real service. Hosted runners are
  administrators with UAC disabled on Windows and have passwordless `sudo` on Unix,
  so a real registration would mutate runner logon state and could pass under
  privileges a real user does not have. The service lane keeps asserting the
  **absence** of elevation in the plan and the **content** of the definition.
- The `#[cfg(unix)]` owner-only permission assertions, typechecked but never
  executed on the Windows authoring host, run explicitly in the native lane. Those
  assertions **did not in fact run** until 2026-09-21, because the lane produced
  zero jobs; they were confirmed by hand in a Linux container instead, where 8
  `paths::` tests execute including the permission ones.
- No workflow references a secret, every job declares `permissions: contents:
  read`, and `persist-credentials: false` keeps the job token out of the working
  tree's `.git/config`.

The workflows and journey are committed and pushed, so the lanes execute on every
push. **The result was first read on 2026-09-21, and every one of the 12 runs had
failed.** The earlier note in this file claimed the repository was private and the
runs API unreadable without a token; it is **public**, and the runs API returns the
runs unauthenticated. The failures were not subtle:

- `native.yml` contained a YAML parse error (`paths:: storage::` left unquoted,
  where `: ` is a mapping separator in a plain scalar), so GitHub created runs with
  **zero jobs**. The native lane never executed a single step while reporting a
  failure with no step to inspect.
- The same step's command was invalid regardless: `cargo test` accepts exactly one
  positional filter, so `paths:: storage::` is a usage error.
- Linux `clippy -D warnings` failed on three items that a Windows compiler cannot
  see: an unused import of a constant used only under `not(unix)`, an unused `mut`
  in a `#[cfg(unix)]` block, and a `verbose_bit_mask` lint that only compiles where
  the Unix permission code does.
- The clean-machine journey asserted the service preview matched `jarvisd"`, which
  holds on Windows (task definitions quote paths) and not on Linux
  (`ExecStart=/path/jarvisd`).

A `rust:1.98-slim-bookworm` container with the pinned toolchain,
`scripts/linux-verify.sh`, now reproduces the CI environment locally and runs the
same commands the lane does. That is what made the last two items findable: no
amount of re-reading Windows-only output can surface a defect in code the compiler
never sees.

Read the Actions tab; the runs API is also readable without a token.

The documentation audit that followed the push also found that `docs/README.md`
had drifted: `ci-gates.md`, `rust-foundation.md`, and `github-actions.md` existed
and were linked from their own section indexes, but were never linked from the
top-level index. A document nobody links to is effectively unreviewed. All three
are now linked, and `scripts/validate-docs.mjs` gained an index-completeness check
with a fail-closed test so the next drift fails the build instead of waiting to be
noticed (`CI-C014`).

## Error and Retry Policy

| Condition | Retry? | JARVIS behavior |
| --- | --- | --- |
| Invalid config/schema/version | No | Keep last valid config; return field-safe diagnostics |
| RNG or credential-store unavailable | No automatic downgrade | Fail closed; offer explicit retry/re-enrollment |
| File lock already held | No | Report daemon already running with safe discovery guidance |
| SQLite BUSY | Bounded, operation-specific | Jittered retry only for idempotent operations; retain transaction-state checks |
| SQLite FULL/IOERR/CORRUPT | No blind retry | Stop affected writes, preserve evidence, require backup/repair workflow |
| Migration checksum/missing migration | No | Refuse startup and preserve database |
| Local HTTP overload/body too large | No | Stable 429/413 error without reading unbounded content |
| Shutdown timeout | No extension by default | Record incomplete drain, reconcile durable work on restart, exit nonzero when unsafe |
| Service manager unavailable | Conditional manual retry | Continue in foreground/portable mode with actionable status |
| Log queue full | No request failure | Count loss, expose degraded observability, retain canonical audit in storage |

## Security Analysis

- Trust boundaries: local HTTP input, config/env/CLI input, filesystem state,
  database bytes, keyring metadata/errors, service definitions, and diagnostic
  fields are untrusted.
- Prompt injection is not relevant to Foundation authorization; no model output
  enters policy or service-manager configuration in this milestone.
- Secret leakage paths include command lines, environment blocks, URL paths,
  SQL logging, tracing fields/span inheritance, panic/debug output, support
  bundles, service files, and temporary config files.
- The health probe exposes only liveness. Readiness and all operational routes
  require local bearer authentication as defined by the local API contract.
- Loopback is not authentication. Caller identity and credential verification
  occur before workspace/resource resolution.
- Filesystem paths are canonicalized and constrained before use; creation uses
  parent-handle/owner checks where platform APIs permit it. Tests include links,
  reparse points, swaps, and permission drift.
- Dynamic SQLite extensions, shell command interpolation, inherited ambient
  provider credentials, and service-manager secret environment variables are
  forbidden.
- No production key, signing identity, public updater, release domain, or
  project license is invented. `OWN-001` through `OWN-005` remain release
  blockers.

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `RF-C001` | Rust 1.98.1 builds the workspace on all five targets | DOCUMENTED, native proof pending | Rust target support | Native CI compile fails |
| `RF-C002` | The domain crate can remain free of Axum, SQLx, and OS adapters | INFERRED | JARVIS dependency direction | `cargo tree -p jarvis-domain` contains one |
| `RF-C003` | SQLx 0.9.0 resolves libsqlite3-sys 0.37.0 and SQLite 3.51.3 | VERIFIED | Registry metadata and vendored `sqlite3.c` | Lock/tree/runtime version differs |
| `RF-C004` | SQLite 3.51.3 contains the WAL-reset repair | DOCUMENTED | SQLite WAL and changelog | Official source or stress test contradicts it |
| `RF-C005` | WAL plus FULL is durable across SQLite-supported power-loss assumptions | DOCUMENTED | SQLite pragma/WAL docs | Crash/power-cut harness loses a committed transaction |
| `RF-C006` | Global body limiting rejects raw and extractor requests over 64 KiB | DOCUMENTED | Axum and tower-http docs | Real route accepts 65,537 bytes |
| `RF-C007` | Cancellation does not mark a durable side effect cancelled after it committed | INFERRED | Tokio semantics and JARVIS durability rules | Race test produces contradictory terminal state |
| `RF-C008` | Standard file locks enforce one cooperating daemon instance | DOCUMENTED | Rust File docs | Second native process acquires the held lock |
| `RF-C009` | Config replacement retains either old or new valid content after a crash | INFERRED | Rust rename and ReplaceFileW docs | Fault test yields absent/partial active config |
| `RF-C010` | Native secret stores never require plaintext fallback | PARTIAL: no keyring adapter yet; `FND-007` uses a documented owner-only file (see the deviation) | keyring and OS store docs | A *silent* fallback that downgrades after a store failure |
| `RF-C011` | A 32-byte OS-random credential verifier can be stored as SHA-256 only | INFERRED | getrandom/SHA-2 plus entropy argument | Threat review finds a feasible offline recovery path |
| `RF-C012` | Secret canaries do not appear in any operator-visible sink | PARTIAL: `FND-005` verifies the file log sink and `FND-013` verifies the support-bundle archive; error text and console sinks still pending | Redaction plan | Seeded sink scan finds a canary |
| `RF-C013` | All three OS service facilities install and remove without elevation | DOCUMENTED, native proof pending | OS service docs | Clean-user test prompts or fails for privilege |
| `RF-C014` | Graceful shutdown drains or records every admitted operation | UNVERIFIED until implementation | Tokio/Axum contracts | Kill/race test loses an admitted operation |
| `RF-C015` | Backup/restore rejects corrupt or FK-invalid snapshots | DOCUMENTED, native proof pending | SQLite backup/integrity docs | Corrupt fixture activates |
| `RF-C016` | Canonical typed IDs serialize as lowercase hyphenated UUIDv7 strings and reject non-canonical input | VERIFIED by unit test | uuid `Uuid` docs | A round trip or rejection test fails |
| `RF-C017` | A jiff-backed timestamp displays RFC 3339 `Z` and normalizes parsed offsets to UTC | VERIFIED by unit test | Jiff `Timestamp` docs | Display output lacks `Z` or a parsed offset is not normalized |
| `RF-C018` | Cancellation racing a committed side effect never reports it as cancelled | INFERRED, test deferred | Tokio cancellation semantics | A race test produces a contradictory terminal state |
| `RF-C019` | The domain crate's only external dependencies are uuid and jiff, both domain value/time types | INFERRED | Dependency direction | `cargo tree -p jarvis-domain` shows another crate |
| `RF-C020` | A resolved profile path can never escape its standard root through a rooted, absolute, or `..` component | VERIFIED by unit test | `directories` mappings plus path-component rule | A crafted profile name resolves outside the root |
| `RF-C021` | Unix state directories are created `0o700` and files `0o600`; `umask` can only tighten them | VERIFIED where Unix | Standard library mode APIs | A directory mode reports any group/other bit set |
| `RF-C022` | A configuration document cannot carry a secret value, only a reference, and no failure message or render contains it | VERIFIED by unit test | `SecretReference` design | A seeded value appears in config text, `Debug`, or an error |
| `RF-C023` | An unsupported `schema_version` is refused and the file is left byte-identical | VERIFIED by unit test | Version probe ordering | The file changes or the version is silently accepted |
| `RF-C024` | An interrupted or repeated atomic write leaves either the old or the new complete document and no temp file | VERIFIED for the success and repeat path; crash injection deferred | Rename semantics and `create_new` | A partial file becomes the destination, or a temp file remains |
| `RF-C025` | A registered secret value never appears in formatted log output, at any level of nesting in a JSON record | VERIFIED by unit test | Sink-level writer design | A canary appears in the emitted line |
| `RF-C026` | A logged value containing a newline cannot produce a second log record | VERIFIED by unit test | Writer line framing | A value produces two records |
| `RF-C027` | The exact reviewed `tracing-appender` version is what the lockfile resolves | VERIFIED | Exact version pin | `cargo tree` shows a version other than 0.2.4 |
| `RF-C028` | Terminal control characters and ANSI escapes from a value are removed from output | VERIFIED by unit test | Sanitization pass | An escape sequence survives redaction |
| `RF-C029` | A database written by a newer binary is refused rather than modified | VERIFIED by unit test | Compatibility-record check | A newer version is read or overwritten |
| `RF-C030` | `integrity_check` alone does not detect a foreign-key violation, so both checks are required | VERIFIED by unit test | SQLite pragma semantics | A dangling row passes both checks |
| `RF-C031` | A backup is verified on the copy before it is reported created, and a corrupt backup cannot replace the live database | VERIFIED by unit test | Verify-before-restore ordering | A corrupt backup overwrites or a failed restore retains it |
| `RF-C032` | `trusted_schema` is off and the connected library reports at least 3.51.3 | VERIFIED by unit test | Read-back of pragma and `sqlite_version()` | The pragma reads 1 or the version is below the floor |
| `RF-C033` | A workspace build can cross-compile to a non-host target | FALSE once storage is included | `cc` build of vendored `sqlite3.c` | Any non-host `cargo check` succeeds without a cross C toolchain |
| `RF-C034` | A local credential is refused unless its SHA-256 verifier matches in constant time, and the stored record never contains the credential | VERIFIED by unit test | Digest design | A stored record or comparison reveals the credential |
| `RF-C035` | The discovery file is never observed at the published path without all of its fields | VERIFIED by unit test | Atomic replace with a pre-generated `instance_id` | A published file is missing a field |
| `RF-C036` | The listener never binds a wildcard address and rejects an unexpected `Origin` | VERIFIED by unit test | Parsed `127.0.0.1` bind plus origin check | A request from a browser origin is accepted |
| `RF-C037` | Drain stops admission and reaches a terminal state within the configured bound | VERIFIED by unit test with a real in-flight request | Bounded drain implementation | An in-flight request is dropped or drain never returns |
| `RF-C038` | The daemon answers an HTTP request over a real socket and unpublishes discovery on drain | VERIFIED by integration test | Bound listener plus graceful shutdown | The port refuses a connection or discovery survives drain |
| `RF-C039` | The CLI refuses to print an unexpected status body | VERIFIED by unit test | Contract-shaped parsing | Arbitrary daemon text reaches the terminal |
| `RF-C040` | No service plan contains an elevation request | VERIFIED by unit test | Exact argument vector per backend | JARVIS runs with elevated rights |
| `RF-C041` | No service definition contains a secret or an environment block | VERIFIED by unit test | Rendered definition inspection | A provider key is copied into a world-readable unit file |
| `RF-C042` | The Windows task records no credential | VERIFIED by unit test | `/ru` and `/rp` absent from the argument vector | A user password is stored in a task definition |
| `RF-C043` | A service field cannot forge an extra definition directive | VERIFIED by unit test | Control-character rejection plus format escaping | A description injects a second directive |
| `RF-C044` | A `LaunchAgent` `Label` matches its plist filename and bootstrap target | VERIFIED by unit test | Rendered label compared to both | The agent cannot be loaded or removed |
| `RF-C045` | A path containing whitespace survives `systemd` argument splitting | VERIFIED by unit test | Quoted `ExecStart` value | systemd starts the wrong program |
| `RF-C046` | XML-significant characters in a plist value are escaped | VERIFIED by unit test | Entity escaping asserted | launchd rejects an unparseable plist |
| `RF-C047` | Captured service-manager output is bounded | VERIFIED by unit test | A command emitting far more than the bound is truncated | A chatty tool exhausts memory |
| `RF-C048` | A missing service-manager program is a typed error, not a panic | VERIFIED by unit test | Absent program name | A diagnostic panics |
| `RF-C049` | The `jarvis service` report names the resolved `jarvisd` daemon | VERIFIED live on Windows | Printed preview names the resolved daemon | The service starts the CLI and never serves |
| `RF-C050` | Service install, start, and removal actually work on each platform | **UNVERIFIED** — native CI only (`FND-010`) | Requires a real per-platform service manager | A plan that is correct as text still fails to register |
| `RF-C051` | A portable profile root isolates a run from the real profile | VERIFIED live on Windows | A clean `--profile` directory is created from nothing and contains only its own state | A clean-machine lane reads or writes the developer's real profile |
| `RF-C052` | The daemon and the client resolve the same profile | VERIFIED by unit test and live run | One resolver, one precedence; both binaries use `--profile` | Client and daemon disagree about which state they are using |
| `RF-C053` | A release build cannot have its profile redirected by the environment | VERIFIED by construction | No environment override exists in the resolver | An environment variable moves durable state, credentials, and discovery of an installed product |
| `RF-C054` | The daemon becomes ready against a profile created by a previous run | VERIFIED live on Windows | The clean-machine journey restarts against the same directory | Restart depends on a fresh directory, or state is lost |
| `RF-C055` | The clean-machine journey leaves no daemon behind on failure | VERIFIED by construction | `finally` block terminates the child and removes the profile | A leaked daemon holds the profile lock and fails a later step misleadingly |
| `RF-C056` | The `#[cfg(unix)]` owner-only permission assertions execute somewhere | DEFINED, execution `UNVERIFIED` | The native lane runs them explicitly on three non-Windows runners | Typechecked-but-never-run assertions are reported as proven |

`RF-C007` and `RF-C018` are not yet executable: durable side-effect commit
records arrive with the storage slices (`FND-006` onward). `FND-002` proves only
the in-process cancellation contract: a parent cancellation is observed by child
tokens, child cancellation does not propagate to the parent, and `cancelled()` is
cancel-safe under a real race.

## Verification Plan

### Gate and dependency checks

- [x] Exact Rust, crate, license, repository, and MSRV metadata reviewed.
- [x] SQLx-compatible bundled SQLite source read as `3.51.3`.
- [x] Broad SQLx `sqlite` feature rejected in favor of `sqlite-bundled`.
- [x] Standard library locking selected instead of `fs4`.
- [x] Initial `cargo tree -e features` contains only the seven workspace
  packages; `jarvis-domain` has no dependencies.
- [x] `cargo tree -i libsqlite3-sys` and runtime `sqlite_version()` match this note.
  Resolved `libsqlite3-sys v0.37.0` from `sqlx-sqlite v0.9.0`; the vendored
  `sqlite3.h` in that crate reports `SQLITE_VERSION "3.51.3"`, which is the first
  release containing the WAL-reset repair.
- [ ] Advisory, license, source, and SBOM scans pass before artifact creation.

### Deterministic tests

- [x] Typed IDs serialize canonically and reject non-canonical input; the domain
  timestamp displays RFC 3339 `Z`.
- [x] In-process cancellation propagation and cancel-safe `cancelled()` races.
- [ ] Versioned JSON round trips, unknown versions, trailing data, deep nesting,
  and numeric boundaries.
- [ ] Cancellation before/during/after persistence and bounded drain timeout.
- [ ] Raw, streaming, compressed, and extractor body-limit cases.
- [x] Profile path resolution, root-escape rejection, and lexical containment.
- [x] Config precedence, unknown-field rejection, unsupported-version refusal,
  bounded size, and secret-reference redaction.
- [x] Redaction of a registered canary and of credential-shaped strings, plus
  control-character sanitization, at the log writer.
- [ ] Owner-only atomic write under an injected interruption, malformed prior
  file, and link swap.
- [ ] Credential generation, digest comparison, keyring unavailable/locked, and
  redaction across every sink.
- [ ] Fresh migration, upgrade, checksum drift, concurrent migration, BUSY,
  FULL, IOERR, corrupt DB, checkpoint starvation, backup, and restore.
  Done so far: fresh migration, idempotent re-run, target-version record, newer-
  schema refusal, integrity versus foreign-key check, lock claim/refusal/expiry/
  release, backup creation and verification, restore, and corrupt-backup refusal.
  Still open: an upgrade from a prior supported schema, checksum drift, `BUSY`
  and `FULL` injection, corrupt-database repair, checkpoint starvation, and
  abrupt-termination recovery.
- [ ] Single-instance contention and stale unlocked-file repair using real
  processes.

### Native gated tests

- [~] Unix: state directories are created `0o700` and files `0o600`, and a
  relaxed directory is rejected. The tests are written and typecheck for
  `x86_64-unknown-linux-gnu`. The `Native targets` lane now runs them explicitly
  with `cargo test -p jarvis-infrastructure paths::` on the three
  non-Windows runners. They remain **unexecuted** until that workflow first runs
  on GitHub's infrastructure, and are not claimed as proven before then.
- [ ] Windows x86_64: Known Folders, owner DACL, Credential Manager,
  ReplaceFileW faults, Task Scheduler lifecycle, SQLite crash recovery.
- [ ] macOS arm64 and x86_64: standard paths/modes, Keychain locked/denied,
  LaunchAgent lifecycle, replacement and SQLite crash recovery.
- [ ] Linux x86_64 and arm64: XDG paths/modes, Secret Service absent/locked,
  systemd user lifecycle, local-filesystem/WAL recovery.
- [~] Clean user homes and source-tree executables: the clean-machine journey runs
  the release-profile binaries against a fresh `--profile` directory rather than
  the developer's real profile, which is a genuine isolation boundary. It is still
  **not** a packaged install: no installer, archive, or signature is involved.
  Packaged clean-machine journeys remain `FND-011` and `FND-012`.

## Operational Readiness

- [x] Kill switch and foreground fallback defined.
- [x] Unsupported deployment modes recorded.
- [x] Retry and failure classes recorded.
- [x] Secret placement and redaction requirements recorded.
- [ ] Native service operator commands and repair runbooks implemented.
- [x] Bounded diagnostics and support-bundle preview implemented (`FND-013`),
  with previewed repair (`FND-014`). The bundle is preview-first, redaction is
  proven by a secret canary over the exported bytes, and both were verified live
  on real binaries. Still missing from a *complete* bundle: trace/metric/health
  summaries, runtime/plugin inventory, and schema-migration state, which need
  the Milestone 2-8 subsystems.
- [ ] Migration, backup, restore, rollback, and uninstall exercised from
  packaged artifacts.
- [ ] Public signing, release, update, and legal gates resolved by the owner.

## Open Questions

- The Windows API binding for owner-only DACL write/query and `ReplaceFileW` is
  deferred. `FND-003` landed without it: Known Folders are already
  ACL-restricted by the OS and JARVIS adds a containment check, so no Windows
  security package was selected. Adding one requires a superseding ADR that
  approves narrowly scoped `unsafe` (or a reviewed safe wrapper) because every
  crate currently forbids `unsafe_code`. That manifest edit must refresh this
  note before code is added.
- Linux Secret Service availability varies by desktop/session. The supported
  distributions are not declared until native clean-user tests run.
- Systemd, launchd, and Task Scheduler runtime versions are supplied by the OS,
  so installer checks must probe capabilities rather than trust version names.
- Production artifact signing, release channels, legal notices, and public
  update metadata remain blocked by `OWN-001` through `OWN-005`.

None of these questions controls `FND-001`; each is a stop condition for its
own later slice if native evidence cannot satisfy the contract.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-21 | Initial implementation-ready Foundation research | Versioned official docs, registry metadata, SDK source, OS specifications, and local toolchain inspection |
| 2026-09-21 | FND-001 workspace verified | Format, Clippy, tests, dependency trees, and five-target compile checks |
| 2026-09-21 | FND-002 ID/time/cancellation/error primitives | jiff 0.2.37 decision, first external dependencies, serialization and cancellation-race tests |
| 2026-09-21 | FND-003 path resolver and permission decision | directories 6.0.0 mappings, Unix mode enforcement, Windows Known-Folder containment, Windows DACL write recorded as deferred |
| 2026-09-21 | FND-004 layered configuration | toml 1.1.6+spec-1.1.0 `parse`/`serde`/`display`, precedence and allowlist decision, secret-reference-only schema, atomic write design, `toml::Value` `unbounded` avoided |
| 2026-09-21 | FND-005 observability and redaction | Sink-level writer redaction with a registered-value guarantee plus best-effort patterns, control-character sanitization, bounded `lossy(false)` queue with a dropped-line counter, `tracing-appender` pinned exactly to the reviewed 0.2.4 |
| 2026-09-21 | FND-006 storage primitives | Verified `libsqlite3-sys 0.37.0` / SQLite 3.51.3 resolves and reports at runtime; `sqlite-bundled` keeps `sqlite-load-extension` out of the graph; `trusted_schema` set off and read back; newer-schema refusal, dual integrity/foreign-key check, scoped expiring locks, and verify-before-restore backup; bundled SQLite's need for a native C toolchain per target recorded as a cross-compilation limitation |
| 2026-09-21 | FND-007 daemon lifecycle and local transport | Startup ordering, loopback-only bind with `Origin` rejection, 32-byte `getrandom` credential with a SHA-256 verifier compared in constant time, atomic discovery publication, separate liveness/readiness, and bounded drain; credential-store storage temporarily replaced by an owner-only file with `FND-008`/keyring as the owner |
| 2026-09-21 | FND-008 CLI and serve loop | Serve loop bound to the router with graceful shutdown and drain; CLI subcommands over a minimal loopback HTTP client with contract-shaped response parsing, typed non-exiting argument parsing, and actionable codes; live end-to-end verified (`jarvis status` against a running `jarvisd`) |
| 2026-09-21 | FND-009 per-user service lifecycle | Plan/execute separation with three controllers (`systemctl --user`, `launchctl gui/<uid>`, `schtasks /sc ONLOGON /rl LIMITED /it`); per-format escaping for unit and plist definitions; `jarvis service` reports state plus the exact install preview and resolves the real `jarvisd`; native registration/removal proofs deferred to `FND-010` |
| 2026-09-21 | FND-010 CI lanes and clean-machine journey | Two workflows (`CI`, `Native targets`) covering the five tier-1 targets on matching runners with `fail-fast: false`; unified profile resolution with an explicit `--profile` root and no environment override; a dependency-free `scripts/clean-machine-smoke.mjs` proving `ACC-001` including restart and a bounded drain; Unix permission assertions executed in the native lane. Full CI evidence in [github-actions.md](github-actions.md). First execution on GitHub's infrastructure remains `UNVERIFIED` |
| 2026-09-21 | FND-013 diagnostics and support bundle | No **new** third-party dependency: the only `Cargo.toml` edit adds the already-reviewed `jarvis-observability` crate to `jarvis-infrastructure`, so the existing package approvals still cover every linked crate. The support bundle is therefore a hand-written stored-ZIP writer in `diagnostics/archive.rs` rather than an archive crate: method 0 only, no ZIP64 (bounded by a 16 MiB total and 64 member cap checked before any byte is written), a fixed 1980-01-01 DOS timestamp for byte-identical output, and a `const` CRC-32 table. Redaction reuses the FND-005 writer guarantee. Verified live: `.NET ZipFile::OpenRead` opened the produced archive (4 members incl. `manifest.json`), and with the live 43-character client credential appended to the real log, the raw file contained it and the exported bundle did not (`presented credential [REDACTED] while enrolling`). The `clean-machine-smoke.mjs` journey now plants a canary and asserts the exported archive omits it; removing the credential registration made that assertion fail, so the check falsifies |
| 2026-09-21 | FND-014 previewed repair plans | No dependency change at all, so no new package approval; the plan/apply design reuses the existing `InstanceGuard`/`appears_unheld` lock primitives and the FND-002/FND-003 path helpers. Three behaviour corrections were found by running the code rather than by reading it, and each had been hidden by a plausible-looking earlier version: the directory check called `ensure_directories`, so a missing-directory fault was **unobservable** because the check created it; an unheld lock file was treated as stale although a clean drain leaves the file behind, making it the daemon's normal resting state; and the repair was keyed on reachability, which reports a dead daemon as running whenever the discovery file survives an unclean kill. The authoritative condition is now a discovery file surviving a **free** lock, verified live after `taskkill /F` where reachability reported `ok` and the lock-derived check caught the stale state. `clean-machine-smoke.mjs` asserts convergence rather than the pre-state, because Windows cannot deliver a graceful stop through `child.kill`, so whether a stopped daemon leaves a stale discovery file differs by platform for a reason that is not a defect |