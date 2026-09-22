# Process Topology

Status: PROPOSED

## Product Processes

### `jarvisd`

The long-running authority. It owns API listeners, application services,
repositories, runtime/tool supervision, event dispatch, scheduling, workflows,
health, and graceful shutdown.

It must be usable in four launch modes:

- foreground development or troubleshooting;
- per-user managed background service;
- portable foreground mode with colocated state;
- server/container mode with explicit remote configuration.

### `jarvis`

A thin CLI client. It may perform bootstrap and offline repair operations that
cannot use a running daemon, but normal product commands call `jarvisd`. It must
not embed a second agent core.

### `jarvis-desktop`

A Tauri client. It may start or discover the local daemon and render OS-native
notifications, tray, microphone, and approval UI. It does not access provider
credentials, databases, or tools directly.

### External Workers

Agent runtimes, plugins, MCP stdio servers, browser automation, ML workers, and
sandboxes normally run in child or remote processes. Each has a narrow protocol,
explicit lifecycle, isolated environment, resource limits, and cancellation.

## Local Communication

V1 should use an authenticated HTTP/WebSocket/SSE API on a dynamically selected
loopback port, recorded in a permission-restricted runtime file. This maximizes
parity across CLI, desktop, and bundled web clients.

Required controls:

- bind only to `127.0.0.1` and `::1`, never wildcard by default;
- generate a high-entropy local bearer credential on first launch;
- store runtime discovery and token material with owner-only ACLs;
- reject missing/unexpected `Host` and browser `Origin` values;
- do not use ambient browser cookies for privileged API auth;
- rotate local credentials during explicit reset/recovery;
- expose readiness separately from process liveness;
- cap requests, streams, connections, headers, and bodies;
- avoid putting tokens in URLs, process arguments, or logs.

Unix domain sockets and Windows named pipes remain valid future transports when
they solve a measured threat or deployment need. The public application protocol
must not depend on loopback-specific semantics.

## Server Communication

Remote mode requires explicit enablement and:

- TLS with a documented reverse-proxy/direct-TLS topology;
- user or service-client authentication;
- scoped, expiring access tokens;
- origin/CORS policy per deployed UI;
- rate limits and abuse controls;
- audit of authentication, token, and policy decisions;
- no reuse of the local bootstrap token.

## Process Supervision

Every managed child process has:

- immutable executable identity and resolved path;
- a sanitized environment allowlist;
- an assigned working directory and capability roots;
- startup and handshake deadlines;
- heartbeat or observable liveness when the protocol supports it;
- bounded stdin/stdout/stderr and message sizes;
- turn, idle, and total runtime deadlines;
- cancellation followed by bounded graceful stop and forced termination;
- restart budget with backoff and quarantine;
- owner, workspace, run, version, and correlation metadata;
- a diagnostic state that does not include secrets.

Child stdout is protocol data only when the protocol says so. Diagnostic output
belongs on stderr or a separate structured channel.

## Service Management

| Platform | Personal default | Server/admin option |
| --- | --- | --- |
| Linux | systemd user service | system service/container |
| macOS | launchd LaunchAgent | LaunchDaemon only for explicit admin installs |
| Windows | per-user Scheduled Task or startup registration | Windows Service with explicit elevation |

The installer must not trigger an elevation prompt it cannot complete. Personal
mode should not require administrator privileges.

Service definitions record the installed binary path, profile, state path, and
minimal environment. They do not embed provider secrets. `jarvis doctor` detects
definition drift, stale binaries, duplicate services, bad permissions, version
mismatch, port conflict, and unhealthy startup.

## Platform Directories

Use an OS-aware path library and preserve a strict distinction between config,
data, state, cache, logs, runtime discovery, and secrets.

| Purpose | Windows target | macOS target | Linux target |
| --- | --- | --- | --- |
| Config | `%APPDATA%/Jarvis` | `~/Library/Application Support/Jarvis` | `$XDG_CONFIG_HOME/jarvis` |
| Durable data | `%LOCALAPPDATA%/Jarvis/data` | `~/Library/Application Support/Jarvis/data` | `$XDG_DATA_HOME/jarvis` |
| Mutable state | `%LOCALAPPDATA%/Jarvis/state` | `~/Library/Application Support/Jarvis/state` | `$XDG_STATE_HOME/jarvis` |
| Cache | OS local cache/Jarvis | `~/Library/Caches/Jarvis` | `$XDG_CACHE_HOME/jarvis` |
| Logs | `%LOCALAPPDATA%/Jarvis/logs` | `~/Library/Logs/Jarvis` | `$XDG_STATE_HOME/jarvis/logs` |

Exact paths must be verified against the selected Rust path crate and installer
behavior before implementation. Portable mode uses an explicit root and never
silently mixes with installed-profile state.

## Startup Order

1. Parse immutable launch options and determine profile paths.
2. Acquire single-instance lock for the profile.
3. Initialize safe stderr logging, then configured tracing/redaction.
4. Load and migrate configuration without resolving provider secrets.
5. Open storage, validate schema, and recover incomplete internal transactions.
6. Initialize identity, policy, repositories, outbox, and scheduler.
7. Bind local API and publish permission-restricted discovery state.
8. Start workers, connectors, runtime supervisor, and workflow recovery.
9. Mark readiness only after required stores and policy are available.

## Shutdown Order

1. Stop accepting new durable work and mark readiness false.
2. Notify clients and cancel non-durable streams.
3. Drain admitted requests for a bounded interval.
4. Persist resumable state and release workflow/job leases.
5. Stop connectors, runtimes, MCP children, and background workers.
6. Flush outbox/checkpoints, telemetry, and logs with time bounds.
7. Remove runtime discovery state and release the instance lock.

Forced termination at any point must recover through persisted invariants, not a
perfect shutdown assumption.

### Implemented evidence (startup order, steps 5–9)

Steps 5, 7, and 9 exist and their **order is asserted**, not merely intended. `start`
opens and migrates storage, then runs the recovery pass over interrupted runs
(`jarvis_application::recovery::reconcile`), and only then publishes the discovery file
and marks readiness. That ordering is the one this document's step 5 asks for and the
local control API requires literally: readiness must stay false until recovery
classification completes, so a client cannot reach a daemon that has not yet settled the
runs it is about to serve.

The pass is a run-level instance of "recover incomplete internal transactions": each
interrupted run is settled to a terminal state with its public event in the same
write, through the same fused repository call every other transition uses. The
recovery count is returned to the composition root rather than logged inside the
storage layer, because startup order, logging, and exit decisions all live in `main`,
and because `jarvis-infrastructure` has no logging dependency of its own.

Steps 2, 3, 4, and 8 are partially implemented: the single-instance lock, stderr
logging, configuration loading, and the scheduler's absence are covered elsewhere in
this document, and steps 8's workers, connectors, runtime supervisor, and workflow
recovery are later milestones. **Not implemented at this step:** no interrupted
external runtime process is reconciled, because no runtime supervisor exists yet.