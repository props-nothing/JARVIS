# Runbook: MCP Servers

Status: ACCEPTED

Scope: a JARVIS daemon (`jarvisd`) that launches one or more configured MCP
servers over stdio. Owner: the daemon surface. Last verified: 2026-10-03, against
the build that ships slices 1–26 of `docs/research/integrations/mcp.md`.

This runbook uses public commands and operator-visible states. Nothing here
requires editing the database, reading a credential, or parsing a stack trace.

## What this surface is, and its one honest limit

A configured MCP server is **a child process the daemon launches**, over stdio,
with an environment the operator declared and no ambient inheritance. Its tools
become canonical JARVIS tools: they pass the same validation, policy, approval,
idempotency, and audit pipeline as a native tool, and the model may propose a call
against one but never authorize it.

A server that dies after a successful startup is **restarted by a supervisor**,
under a bound. Every 5 seconds the daemon looks for a closed server and
relaunches it from its declaration (secrets are resolved again at that moment).
Attempts back off from 1 s, doubling to 60 s, and after 5 consecutive attempts the
server is **quarantined** until the daemon restarts. The count is forgotten only
once a restarted server has stayed up for 60 s, so a server that dies straight
after every start cannot reset its own budget.

Two limits remain. A restarted server is accepted only if it offers **exactly**
the tools it did before; one that comes back with a different catalog is
quarantined rather than substituted, because the grants and approvals were
written against the old one. And liveness is the session's own `closed` state:
a server that is still open but hung is not detected. A *call* that raced a death
is still reported unsettled, and is never retried automatically.

## Symptoms and safe diagnostics

Read the daemon's own view first. It is authenticated and reads the live
composition, so it answers "which of my servers are up **now**":

```bash
jarvis status
```

```
mcp servers: 2
  acme-files           tools=4 refused=0 closed=false
  other-files          tools=1 refused=3 closed=true
mcp refused: 1
  third-files          code=mcp.spawn_failed permanent=false
```

Three readings, and they mean different things:

| Reading | Meaning | Correction |
| --- | --- | --- |
| `closed=true` on a row | The child is gone — it died, or its transport ended | Wait one supervision pass; if it stays closed see "A server died during operation" |
| `refused=N` on a row | The server offered more tools than it contributed | Read the per-tool lines below; the server is *running* |
| A row in `mcp refused` | The server never composed | Its `code` names the correction |

For **per-tool** refusals — the ones that do not stop a server from running —
read the daemon's log at startup:

```bash
journalctl --user -u jarvisd | grep 'mcp'
# or, for a foreground run:
jarvisd | grep 'mcp'
```

| Log line | Meaning |
| --- | --- |
| `mcp server refused at startup: server=… code=… permanent=…` | The server did not compose. `permanent=false` means the composition **already re-attempted it once** and it failed the same way |
| `mcp tool refused at startup: server=… tool=… code=…` | A tool the server offered is not callable. The server itself is fine |
| `mcp server registered: server=… added=… unchanged=… refused=…` | `debug` level. The counts, for "did this server end up with the tools it listed" |
| `mcp server was already closed before the drain` | A server was closed at shutdown — quarantined, or between restart attempts |
| `mcp server restarted: server=… attempt=…` | The supervisor relaunched a dead server; its tools are callable again |
| `mcp server restart failed: server=… code=… attempt=… retry_in_ms=…` | A restart attempt failed in a way a later one might fix |
| `mcp server quarantined until the daemon restarts: server=… code=…` | Given up on. `mcp.restart_budget_exhausted`: it kept dying. `mcp.restart_identity_changed`: it came back with different tools — review them before restarting. Other codes are permanent launch or catalog faults |
| `mcp server startup failed transiently, retrying` | A transient failure is being re-attempted. One retry, 250 ms apart |

A **configuration** fault fails startup instead of appearing here, and reports
`jarvis.config_mcp_invalid` naming the field: an unusable server name, a program
that is not a usable token, too many arguments or variables, an environment key
with a `=` in it, or a duplicated name. Fix the profile; the daemon refuses to
start with a declaration it cannot honour.

## Using a server's tools: discovery is not permission

A composed server's tools join the daemon's tool catalog, so the model is **offered** them with their real
description and argument schema. Offering is all that happens: a call is refused `tool.permission_denied` until
an operator writes a grant for it, and then every call still waits for a human approval, because an MCP server's
annotations can propose but never permit.

```bash
# An MCP tool with no annotations is recorded as `write` at `moderate` risk (the absence of a read-only claim is
# not a claim of safety). A grant may only narrow, so it names exactly that.
jarvis grants create --capability mcp.read_file@1 --principal <your-principal-id> \
  --effect write --risk moderate --sensitivity internal
jarvis approvals list          # a model's call to the tool appears here
jarvis approvals approve <id> --fingerprint <fp> --version <n>
```

A decided approval **continues the run**: approved, the tool runs in the server and the model answers from its
result; rejected or withdrawn, the model is told the call was refused and the tool never runs. An approval is
spent by the call it authorizes, so the same action asks again. A run still waiting when the daemon restarts is
failed, not resumed (`docs/architecture/agent-runtime.md`, "Resuming after an approval"). One more limit to know: a
client cannot ask the daemon for its own principal id today; for the enrolled `owner`
client it is the first sixteen bytes of the SHA-256 of `owner`, written as a UUID. A secret reference in a server's
`env` must begin with `JARVIS_` (for example `env:JARVIS_ACME_TOKEN`); a differently named one makes the
configuration unparseable (`jarvis.config_parse`, recorded in the daemon's log file, not on the console).

## Containment: stop a server without stopping the daemon

There is no per-server disable command. The kill switch is the declaration:

```toml
[[mcp.servers]]
name = "acme-files"
program = "/usr/local/bin/acme-mcp-server"
enabled = false          # ← the whole switch
```

`enabled = false` is **validated like any other declaration** — the daemon still
checks the name, program, and environment keys — because a disabled server is a
reviewed record that has to stay correct. It is never spawned, and its secret is
never resolved. Restart the daemon to apply.

A server that is misbehaving but must stay enabled can be contained with a
`[tools] deny` rule instead, which refuses its tools at the policy layer without
touching the launch:

```bash
jarvis grants deny add --effect write --reason "containing a misbehaving server"
```

The model data policy is a **different** surface and does not gate MCP tools: it governs which model a call
may route to, not which tool may run. Read it with `jarvis policy show`; the write is narrowing-only and
requires the version the read printed:

```bash
jarvis policy show
jarvis policy put --name strict --expected-version 3 \
  --locality local_only --retention none_documented --training-use disallowed_documented \
  --telemetry disabled --sensitivity confidential --fallback denied --region eu
```

## Recovery

### A server never composed

1. Read its `code` from `jarvis status` or the startup log.
2. Match it to the correction below.
3. Fix the profile or the environment, then restart the daemon.

| Code | Correction |
| --- | --- |
| `mcp.spawn_failed` | The program could not be launched. Check the path exists and is executable **by the daemon's user** |
| `mcp.startup_no_compatible_version` | The server implements no protocol revision JARVIS offers. Upgrade one side; a retry cannot help |
| `mcp.server_config_invalid` | The configured name is unusable. A name must satisfy both the configuration-identity and tool-owner rules — lowercase, ≤128, and **starting with a letter** |
| `mcp.discovery_timeout` | The server did not answer `tools/list` within 60 s. Usually a server waiting on input it will never get |
| `mcp.no_usable_tools` | Every tool it listed was refused. Read the per-tool lines; the codes there name why |
| `mcp.catalog_empty` | The catalog was admitted but nothing survived publishing. Two servers may be claiming one capability |
| `jarvis.secret_unavailable` | An environment reference could not be resolved. The daemon reads the variable from **its own** environment |
| `mcp.working_directory_invalid` | The declared `working_directory` is unusable — empty, over 512 bytes, or carrying a control character |
| `mcp.identity_changed` | A tool's schema moved under an identity approvals are recorded against. Re-approve the new identity deliberately |

### A tool is offered but not callable

Find its per-tool line. `mcp.tool_name_invalid` and `mcp.schema_rejected` mean the
server shipped something JARVIS will not guess at: a name that is not a usable
canonical segment (spaces, uppercase), or an input schema using a keyword this
workspace does not implement. Neither is repaired by retrying — the server has to
change what it lists.

An **all-refused** catalog is refused as a server, so an operator sees
`mcp.no_usable_tools` rather than a server running with no tools. A **partial**
refusal leaves the server running, which is why the per-tool line exists: the
server is fine and one of its tools is not.

### A server died during operation

`jarvis status` reports `closed=true`. The supervisor relaunches it within a few
seconds; a `restarted` log line confirms it. If the row stays closed, look for a
`quarantined` line, fix the cause, and **restart the daemon.** The daemon's drain stops every remaining server inside the runtime and
reports any it could not confirm stopped (`mcp server did not stop cleanly`), so a
restart is a clean stop rather than an orphaned child.

Before restarting, check whether the server is repeatedly dying — a child that
exits immediately on every start is a configuration problem wearing a runtime
symptom.

## Risks

- **A server without a declared `working_directory` runs in the daemon's own
  working directory.** That is a real exposure: a server launched from wherever
  `jarvisd` started can open relative paths it was never granted. Declare one:
  ```toml
  working_directory = "/var/lib/jarvis/mcp/acme"
  ```
  It is optional rather than required because a default would have to be invented
  — there is no directory JARVIS may assume is appropriate for a third-party
  server — so the absence is a decision an operator makes, not one the adapter
  makes for them.
- **A server's stderr is retained in a bounded tail** (16 KiB) and its `Debug`
  prints the size, never the content. Do not paste it into a ticket unredacted;
  it is untrusted text a third party chose.
- **A server's tool annotations cannot lower its effects or risk**, and a lying
  `readOnlyHint` buys a label rather than an approval. If a server claims a tool
  is read-only, that claim is not what authorizes it.
- **Every tool call still passes policy and approval.** A server cannot be given a
  grant by being trusted; grants name an identity, and a schema change produces a
  different identity.

## Verification

From the user-facing surface, after a change:

1. `jarvis status` shows the servers you expect, with `closed=false`.
2. `jarvis status` shows a row per server you deliberately disabled — it should
   **not** appear in `mcp refused`, because a disabled declaration is skipped
   before anything is attempted.
3. A tool call through a granted tool succeeds (`jarvis ask "…"`), which proves
   the dispatch path rather than only the health line.
4. On shutdown, the log carries `stopped N mcp server(s) during drain` and no
   `did not stop cleanly` line. A non-zero unclean count is evidence a server
   ignored its cancellation.

## Escalation and evidence to retain

Retain, after redaction: the refusal **codes** and server names (never the refusal
messages, which may carry operator text), the daemon log lines above, the profile's
`[[mcp.servers]]` entries **with environment values removed**, and the version
from `jarvis status`.

Do not retain: the child's stderr tail verbatim, any resolved environment value,
or a credential reference's value. Environment values are resolved only at the
moment of launch and never appear in a record.

## Automated scenarios

| Procedure | Scenario |
| --- | --- |
| A declared server becomes a callable tool | `mcp_composition_process::a_declared_server_becomes_a_callable_tool_through_the_whole_chain` |
| A refused server does not deny the others | `mcp_composition_process::a_server_whose_secret_is_missing_is_refused_while_the_others_compose` |
| A disabled server is never spawned | `mcp_composition_process::a_disabled_server_is_never_spawned` |
| A partial refusal stays visible beside the admitted tool | `mcp_composition_process::a_partially_refused_catalog_is_reported_beside_the_tool_that_was_admitted` |
| A dead server is reported rather than silently dispatched to | `mcp_composition_process::a_lifecycle_probe_records_each_servers_health` |
| A declared working directory is the one the child runs in | `mcp_composition_process::a_declared_working_directory_is_the_one_a_spawned_child_actually_runs_in` |
| The drain stops every child | `mcp_composition_process::shutting_a_composition_down_closes_every_session` |
| The status surface reports servers and follows their health | `http::tests::the_status_surface_reports_mcp_servers_and_follows_their_health` |
| A transient startup failure is retried once | `mcp::composition::tests::a_transient_startup_failure_is_retried_once_and_then_reported` |
| The reference client can read the MCP half | `jarvis-cli` `tests::the_mcp_half_of_a_status_body_parses_with_its_refusals` |
