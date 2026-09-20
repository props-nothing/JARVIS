# Operations Documentation

Status: ACCEPTED

Operations documentation turns architecture into repeatable build, release, and
recovery procedures. A command is not presented as usable until its owning TODO
has implementation evidence.

## Current Handoffs

- [Foundation implementation handoff](foundation-handoff.md)
- [Release readiness and owner decisions](release-readiness.md)

## Required Before Each Product Surface Ships

The owning milestone must add or update runbooks for:

| Surface | Required runbook evidence |
| --- | --- |
| Daemon and local storage | startup, shutdown, lock recovery, corruption, backup, restore |
| Installer and updater | clean install, repair, interrupted update, rollback, uninstall |
| Model provider | setup, invalid auth, limits, outage, disable, credential rotation |
| Connector | setup, reauth, health, sync lag, webhook failure, disable, delete |
| MCP/runtime/plugin | install, scope grant, health, quarantine, upgrade, removal |
| Workflow | stuck lease, retry exhaustion, approval wait, cancellation, replay |
| Voice/telephony | provider outage, identity failure, callback replay, call stop, cost kill switch |
| Server mode | database recovery, key rotation, tenant incident, capacity, disaster recovery |

Runbooks use public commands and operator-visible states. They must not require
editing the database, extracting secrets, or guessing from internal stack traces.

## Runbook Template

Every runbook states:

1. scope, supported versions, owner, and last verified date;
2. symptoms and safe diagnostics;
3. immediate containment and kill switch;
4. ordered recovery steps with preconditions and rollback;
5. data-loss, side-effect, cost, and security risks;
6. verification from the user-facing surface;
7. escalation and evidence to retain after redaction;
8. the automated scenario that exercises the procedure.

Never put live credentials, private keys, customer payloads, or unredacted
support bundles in a runbook or its fixtures.
