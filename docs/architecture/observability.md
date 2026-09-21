# Observability, Health, and Diagnostics

Status: PROPOSED

## Goals

Observability must answer what happened, where time/cost went, which policy made
a decision, whether data is fresh/complete, and how to repair failure without
requiring private payloads or credentials.

## Correlation Model

Propagate as applicable:

```text
request_id
correlation_id
causation_id
trace_id/span_id
principal_id (safe internal ID)
workspace_id (safe internal ID)
session_id
conversation_id
run_id/step_id
workflow_id/workflow_run_id
model_call_id
tool_call_id
approval_id
connector/account ID
runtime instance/run ID
voice call ID
```

External/provider IDs are labelled and never confused with canonical IDs.

## Logs

Use structured events with timestamp, severity, target/subsystem, message code,
safe fields, and correlation IDs. Human text is secondary to stable machine
codes.

Logging rules:

- secrets and authorization material are never accepted as normal fields;
- payload logging is off by default and bounded/redacted when explicitly enabled;
- untrusted strings are sanitized for terminal/log control characters;
- error chains pass through redaction before export;
- file logs rotate by size/time with retention and disk-pressure behavior;
- audit, diagnostic, and debug logs have distinct semantics;
- dropped logs and exporter failures are observable without recursive flooding.

## Traces

OpenTelemetry spans cover:

- gateway/auth and application use case;
- context build and retrieval stages;
- runtime and model calls;
- policy/approval decision latency;
- tool and connector calls;
- workflow step and event handling;
- database transaction/query classes;
- voice turn stages, with **end-of-turn detection as its own span** rather than
  part of a combined turn span. End-of-turn time is additive to perceived
  latency, so merging it into the total removes the only signal that responds to
  a turn-detection change.

Do not attach raw prompts, emails, documents, tool output, transcripts, or secret
headers by default. Content tracing requires explicit development policy and
must be impossible to enable accidentally in protected deployments.

Trace propagation to external runtimes/MCP uses negotiated safe metadata and
never grants access. Untrusted external trace IDs cannot overwrite local trace
ownership without validation.

## Metrics

Initial metrics include:

### Product

- active sessions/runs/workflows/calls;
- run completion/failure/cancel/wait counts and duration;
- approval requested/approved/rejected/expired and wait duration;
- proactive actions prepared/executed/denied.

### Models and Runtimes

- requests, first-token latency, total latency, tokens, estimated cost;
- route/fallback/retry/refusal/error counts;
- runtime startup, crash, restart, quarantine, protocol mismatch;
- bounded queue depth and dropped/overflow events.

### Tools and Connectors

- discovered/exposed/called/allowed/asked/denied tools;
- tool latency, timeout, cancel, ambiguous outcome, retry;
- connector health, auth state, rate-limit remaining/reset;
- webhook valid/invalid/duplicate/lag and sync cursor freshness/completeness.

### Storage and Workflows

- pool saturation, transaction/query latency by operation class;
- migration/backup/restore state;
- outbox/inbox lag, retries, dead letters;
- scheduler lateness, lease contention, workflow wait/retry duration.

Labels must be bounded. Never use user input, tool arguments, URLs, email
addresses, phone numbers, provider error text, or IDs with unbounded cardinality
as metric labels.

## Health Model

- **Liveness**: process event loop/responding; no external dependency required.
- **Readiness**: required storage, policy, migrations, and listeners can serve
  admitted requests.
- **Subsystem health**: models, runtimes, connectors, event dispatcher, scheduler,
  object/secret stores, and exporters report independent states.

Health states use `healthy`, `degraded`, `unavailable`, `disabled`, and
`unknown`, plus last checked/success, safe reason code, and next action. One
optional provider outage should not make daemon liveness fail.

## `jarvis doctor`

Doctor is deterministic diagnostics and optional explicit repair, not an LLM
guess. It checks:

- binary/daemon/CLI versions and API compatibility;
- service definition, executable drift, duplicates, permissions, and startup;
- config schema/migration and invalid/unknown fields;
- config/data/state/cache/log/runtime path ownership/ACLs and disk space;
- single-instance locks, ports, loopback auth, TLS/remote exposure;
- SQLite/PostgreSQL integrity, migrations, backup readiness, and pool health;
- secret-reference presence/access without printing values;
- model/runtime/MCP/connector configuration and scoped probes;
- scheduler, outbox, dead letters, stuck leases, and resumable runs;
- update channel/signature state and newer compatible release;
- redaction self-test using generated canary values.

Repair is plan-first: show changes, require confirmation for mutations, back up
state/config, use locks, and verify postconditions. Doctor never weakens auth or
deletes user data as an automatic fix.

### Implemented repair (Milestone 1)

`jarvis repair` previews a plan and applies it only with `--confirm`. A plan is a
pure value: it names a diagnosis, the exact actions and the path each touches, and
whether it removes anything. `apply` refuses an unconfirmed plan, a path outside
the profile, a path that would remove user data, and a plan it cannot verify; it
checks the postcondition before applying and again afterwards, rolling back what it
changed. Findings with no safe repair are printed, so "nothing to repair" is never
mistaken for "nothing is wrong".

The repairable conditions today are a missing profile directory and a stale daemon
runtime state. The stale condition is deliberately precise: a **discovery file that
survived a free instance lock**. A clean drain releases the lock but leaves the lock
file on disk, so an unheld lock is the daemon's normal resting state and removing it
would fix nothing; conversely the discovery file is only a file, so a check that
trusted it reports a dead daemon as running. The lock is the authority for liveness
and the discovery file is the evidence of a missing drain.

Not repairable by design: corrupt or unsupported storage (that is a restore, and a
repair that rewrote data would be deleting user data), a missing credential (it
requires the daemon to enroll), configuration content, and service-definition
drift. A plan is also not atomic against a daemon that starts mid-apply.

### Daemon reachability is probed, not inferred

The `daemon` check does not read the discovery file to decide whether a daemon is
running. A published file is not evidence that a process exists: it outlives an
unclean kill and still parses, so a check that reasoned from it alone reported
`ok daemon: running` in the same report where the lock-based check correctly
reported the stale state — two findings that disagreed, and the more
confident-sounding one was the false one.

Reachability is therefore answered by `GET /health/live`, the unauthenticated
liveness route the local control API already defines, which returns only a status
token. The answer is a three-state value rather than a boolean, because the states
need different operator actions:

- `jarvis.daemon_live` — the daemon answered its own liveness route;
- `jarvis.daemon_unreachable` — nothing is listening at the published address;
- `jarvis.port_conflict` — something is listening and is **not** the daemon.

A conflict is distinct because "another process holds the published port" is not
fixed by starting the daemon again, and reporting it as "not running" would send an
operator to the wrong action. It is a warning rather than a blocking finding for
the same reason a stopped daemon is: restarting binds a new ephemeral port, so the
profile recovers without intervention.

### A registered service is not a working service

`ACC-003` seeds a **service-path** fault. `jarvis doctor` therefore does not stop
at the registration state: after an update that moved the version directory, a
registered unit, plist, or task still names the old executable, so it reports
`installed` and serves nothing. The `service` check reads the **installed
definition** and compares it against the executable this build would register.

Reading what is on disk is the only way to see this. Comparing the intended
definition to itself always agrees, and the registration state is not the working
state. Drift is reported with both paths, because an operator needs to know which
executable is registered and which one is current to choose between reinstalling
and rolling back.

The three answers are deliberately distinct, and collapsing them is a bug:

- no definition installed — nothing to compare, so no drift is claimed;
- a definition naming the expected executable — a match;
- a definition that exists but cannot be read — a fault in its own right, never a
  match.

The last one is why **Windows reports no drift rather than no problem**: a
scheduled task is defined by its command line and exposed only through
`schtasks /query /xml`, so with the current dependency set there is no file to
read. That is a named gap, and the check reports the registration state it did
verify rather than a path claim it did not.

## Support Bundle

A support bundle is generated locally and previewed before export. It includes:

- manifest, JARVIS/platform versions, profile mode, and time range;
- redacted config shape and feature flags;
- doctor results and subsystem health timeline;
- bounded structured logs/traces/metrics summaries;
- schema/migration state and non-sensitive runtime/plugin inventory;
- user-selected run IDs with content excluded by default;
- redaction report and hashes.

It excludes secret values, authorization headers, raw environment, full database,
prompts, messages, documents, tool payloads, transcripts/audio, and personal
identifiers unless the user explicitly selects a separately warned attachment.

### Implemented subset (Milestone 1)

The Foundation slice in `jarvis_infrastructure::diagnostics` implements the parts
of this section that can exist before the model, tool, connector, workflow, and
voice subsystems do:

- `jarvis doctor` runs named checks over the profile directories, the database
  (integrity, SQLite version, schema compatibility), daemon reachability, the
  enrolled credential, the configuration file, and service registration. Each
  finding carries a stable check name, a severity, a bounded fact, and fixed
  advice, and warnings are counted separately from blocking findings. A daemon
  that is simply not running is a **warning**, because foreground and portable
  use are supported and a healthy profile must not exit non-zero. Reachability is
  established by probing the daemon's unauthenticated liveness route, never by
  reading the discovery file, which outlives an unclean kill (see
  [daemon reachability](#daemon-reachability-is-probed-not-inferred)).
- `jarvis support-bundle` is preview-first. The plan is rendered before anything
  is written, a bare invocation writes nothing, `--exclude` names optional items,
  and the required manifest is refused by name rather than silently dropped.
- An export is one library call so the manifest's `content_sha256` is guaranteed
  to describe the archive that is written; the archive is a dependency-free
  stored ZIP with a fixed timestamp, so the same inputs produce identical bytes.
  Log tails are bounded to 256 KiB per file over at most 5 files and begin on a
  record boundary, and every member passes through the log writer's redactor.
- Excluded classes are recorded in the manifest as
  `excluded_classes`, not only asserted in prose.

Not implemented yet, and therefore not claimed: trace/metric/health summaries,
runtime and plugin inventory, schema-migration state in the bundle, a
user-selected run attachment path, a redaction self-test command, and
retention/cleanup of previously exported bundles. Those need the owning
subsystems and their TODO IDs.

## Cost and Privacy

Observability itself has retention, storage, network, and provider cost budgets.
Local mode defaults to local observability with no cloud export. Remote exporters
are explicit connectors with data classification, backpressure, failure, and
redaction policies.

## Testing

- seeded canary secrets never appear in any sink or support archive;
- parent/child correlation survives async tasks and process protocols;
- exporter failure and slow sink do not deadlock core work;
- metric cardinality remains bounded under adversarial input;
- health reflects degraded optional versus unavailable required dependencies;
- doctor detects seeded config/service/storage failures and verifies repair;
- support bundle allowlist and preview match archived contents.