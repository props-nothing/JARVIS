# Migration Strategy

Status: PROPOSED

## Goals

- Never silently run an older binary against newer unsupported state.
- Never report readiness during a partial/failed required migration.
- Make local upgrades recoverable without database expertise.
- Preserve server rolling-deployment options only when explicitly supported.
- Test real prior-version artifacts and realistic data, not empty schemas alone.

## Migration Sets

Maintain separate ordered migration directories for SQLite and PostgreSQL with a
shared semantic migration ID and description where they implement the same
domain change.

```text
migrations/
  sqlite/
    000001_initial.sql
  postgres/
    000001_initial.sql
```

Each migration has an immutable checksum. Editing an applied migration is an
error; add a new migration.

## Application Compatibility Metadata

The binary declares:

- minimum readable schema version;
- maximum readable schema version;
- target schema version;
- minimum compatible persisted payload versions;
- whether mixed old/new server processes are allowed.

Startup behavior:

| State | Behavior | Verified by |
| --- | --- | --- |
| DB older, auto-migration safe | Acquire lock, backup/preflight, migrate, verify | `a_fresh_database_migrates_and_reports_the_target_version` |
| DB older, explicit approval required | Readiness false; show command/impact | not implemented — `start` always migrates |
| DB newer than binary | Refuse writes/start, preserve state, explain update | `a_database_written_by_a_newer_binary_is_refused_and_left_untouched` |
| Checksum mismatch | Refuse readiness; require diagnosis | `a_checksum_mismatch_is_refused_rather_than_applied_over` |
| Partial/nontransactional migration | Enter repair-required state | not implemented |

The third column was added because three of these five rows had **no evidence** while the table
read as a specification of behaviour: the refusal existed in code and nothing drove it, so "refuse
writes/start" and "preserve state" were both unverified claims. Two are now asserted against a real
database — the newer-than-binary case checks that `start` refuses **and** that the compatibility
record still names the future version, which is the half that matters to someone who downgraded a
binary. The two rows still marked unimplemented are not defects to fix later by writing a test:
`start` migrates unconditionally, so there is no branch for "explicit approval" or "repair-required"
to take, and a test of behaviour that does not exist would be the false evidence this column exists
to prevent. **A table of states with no way to tell which are real is how a reader concludes a
control exists.**

## Local Upgrade Flow

1. Stop or drain the daemon. Single-instance ownership is the held file lock at
   `<runtime>/jarvisd.lock`, taken by `jarvisd` itself before any startup work; stopping the
   daemon **is** the step, and there is nothing further for an operator to acquire.
2. Verify available disk, database integrity, migration checksums, and supported
   source version.
3. Create and verify a pre-migration backup when required.
4. Apply migrations transactionally where SQLite supports the operations.
5. Run postconditions and application-level invariant checks.
6. Record binary/schema/payload compatibility.
7. Start daemon and run focused readiness/doctor checks.
8. On failure, restore only through the documented safe path; retain failed copy
   for diagnostics with user consent.

**Step 1 used to read "acquire profile maintenance lock", and that instruction could not be
carried out.** It described the `application_locks` table, which `000001_initial.sql` creates —
and a `storage::lock` module implemented the row-level lease on it, with tests for acquisition,
refusal, lease reclamation, and owner-checked release. **Nothing in the product ever called it**,
so there was no command an operator could run and no way to discover that the step was
unperformable. The table and the module were removed; the guard that actually prevents two
daemons from sharing a profile is the held file lock named in step 1, which is real, is taken
before startup work, and is asserted by the daemon's own startup tests. The row-level lease is
the *coordinating* half (a crashed holder, a visible expiry) and remains unimplemented; if a
later slice needs it, it should be built with a caller rather than kept in case.

## Server/Rolling Flow

Use expand/migrate/contract when rolling compatibility is required:

1. Expand schema additively.
2. Deploy code that reads old/new and writes compatible state.
3. Backfill with resumable bounded jobs and progress metrics.
4. Switch reads/writes after verification.
5. Contract old schema in a later release after old binaries are unsupported.

Do not claim rolling support for a migration unless old/new binary integration
tests run concurrently against it.

## Data Transformations

Large transformations use resumable application jobs rather than one unbounded
transaction. Track cursor, batch, attempt, checksum/count, error, and completion.
Dual-read/write periods have explicit end conditions and reconciliation.

## Persisted Payloads

JSON/event/runtime/workflow/memory payload versions require typed decoders and
migration/upcast tests. Database column availability does not prove payload
compatibility.

Unknown security-critical fields/versions fail closed. Noncritical additive
fields can be preserved/ignored according to contract.

## Down Migrations and Rollback

Down migrations are not assumed safe. Release rollback distinguishes:

- binary rollback with unchanged compatible schema;
- forward-fix using a newer binary;
- restore pre-migration backup to a new profile;
- explicitly tested down migration.

The updater must know which category applies before activation. Irreversible
migrations require explicit warning and verified backup.

## Tests

For every supported prior release/profile:

- migrate populated realistic database;
- interrupted migration at available fault points;
- no disk space and locked database;
- checksum drift and newer-schema rejection;
- backup restoration and hash verification;
- old/new serialization fixtures;
- server concurrent old/new process only where supported;
- backfill pause/resume/retry and reconciliation;
- migration runtime/disk bounds at representative scale.

Release CI installs the prior packaged version, creates state through public
interfaces, upgrades with the packaged candidate, and verifies behavior.