// Tool-call recovery journey.
//
// Proves the *composition-root* half of `TLS-006`'s startup recovery: that a real `jarvisd`, on a real
// profile, **settles** a tool-call reservation left stranded by a previous shutdown. The unit tests in
// `jarvis_application::tool_recovery` and `jarvis_infrastructure::storage::tool_call_repository` build
// their own repository and their own `RunPorts`, so they prove the pass and its predicate work when
// called — they cannot prove the daemon calls it, with the ledger repository wired to the same pool, at
// the right point in startup. The composition root is exactly the layer a unit test is structurally
// blind to, which is why this file exists.
//
// What it seeds and what it asserts, in one sentence each:
//
//   1. A **pre-dispatch** reservation (`reserved`) is written into the profile's own database, as a
//      previous daemon would have left it after claiming the key and then crashing.
//   2. The daemon is started. On the way up it must find that row, and because nothing reached the
//      provider it must end it `cancelled` — with the `tool.cancelled` outcome a terminal row requires.
//   3. A **dispatched**, unsettled call (`executing`) must be moved to `reconciling`, which is the state
//      that tells a later retry *the outcome is unknown and must be established* rather than *wait*.
//   4. A terminal row is left exactly as it was, and a second start changes nothing — so recovery is
//      safe to run on every restart.
//
// **Why the `reserved` case carries the weight.** The pass's first version selected `executing` alone,
// so a stranded pre-dispatch reservation was never offered to it: the dead holder kept the unique-index
// entry and every later retry of that key was answered `InFlight`, which is *wait on a process that is
// gone*. That was invisible to every unit test, because the test double mirrored the same narrow
// predicate. Only a real database plus a real startup can show which rows the predicate actually
// returns.
//
// It also asserts the fourth defect of the same kind: the daemon computes the tool-call recovery report
// and, before this round, **dropped it** — the run pass's summary was logged and the tool-call one was
// not, so an operator could not learn from the daemon that the previous shutdown had stranded work. The
// report is now logged, and this harness asserts the log line names the counts.
//
// Dependency-free (Node standard library only), matching the other harnesses in this directory.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const SHUTDOWN_TIMEOUT_MS = 20_000;

let failures = 0;

function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail) {
    console.error(String(detail).slice(0, 2000));
  }
}

function pass(message) {
  console.log(`ok    ${message}`);
}

/** Paths to the two binaries, resolved from a build directory. */
function resolveBinaries() {
  const directory = process.argv[2];
  if (!directory) {
    throw new Error("usage: tool-recovery-journey.mjs <directory containing jarvis and jarvisd>");
  }
  const suffix = process.platform === "win32" ? ".exe" : "";
  const daemon = join(directory, `jarvisd${suffix}`);
  const client = join(directory, `jarvis${suffix}`);
  for (const path of [daemon, client]) {
    if (!existsSync(path)) {
      throw new Error(`missing built binary: ${path}`);
    }
  }
  return { daemon, client };
}

function run(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: 60_000,
    windowsHide: true,
  });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

async function sleep(milliseconds) {
  await new Promise((resolve) => setTimeout(resolve, milliseconds));
}

/**
 * Starts the daemon, waits for readiness, and returns the child **and the log offset it began at**.
 *
 * The daemon writes its `tracing` output to a **file** under `<profile>/log/`, not to stdio, so the
 * offset is what makes "what did *this* start report" answerable — the file is appended to across every
 * start, and reading it whole would let an earlier start's line satisfy a later start's assertion. The
 * offset is captured before the process is spawned so no line can slip between the two.
 */
async function startAndWait(daemon, client, profile) {
  const logOffset = currentLogLength(profile);
  const child = spawn(daemon, ["--profile", profile], {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  const captured = [];
  child.stdout.on("data", (chunk) => captured.push(String(chunk)));
  child.stderr.on("data", (chunk) => captured.push(String(chunk)));

  let exitedEarly = false;
  child.once("exit", () => {
    exitedEarly = true;
  });

  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (exitedEarly) {
      throw new Error(
        `the daemon exited before becoming ready\n${captured.join("")}\n${logSince(profile, logOffset)}`,
      );
    }
    const status = run(client, ["--profile", profile, "status"]);
    if (status.status === 0 && /state:\s*ready/.test(status.stdout)) {
      return { child, logOffset };
    }
    await sleep(POLL_INTERVAL_MS);
  }
  throw new Error(`the daemon was not ready within ${READY_TIMEOUT_MS}ms`);
}

/** The byte length of the profile's newest log file, or 0 when none exists yet. */
function currentLogLength(profile) {
  const path = newestLogPath(profile);
  if (!path) {
    return 0;
  }
  return readFileSync(path, "utf8").length;
}

/** The newest log file in the profile's log directory, or `null`. */
function newestLogPath(profile) {
  const directory = join(profile, "log");
  if (!existsSync(directory)) {
    return null;
  }
  const names = readdirSync(directory).filter((name) => name.endsWith(".log"));
  if (names.length === 0) {
    return null;
  }
  names.sort();
  return join(directory, names.at(-1));
}

/** The profile's log content written since `offset`. */
function logSince(profile, offset) {
  const path = newestLogPath(profile);
  if (!path) {
    return "";
  }
  return readFileSync(path, "utf8").slice(offset);
}

/** Stops the daemon and waits for the process to exit. */
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  const deadline = Date.now() + SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      return;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  child.kill("SIGKILL");
  throw new Error(`the daemon did not stop within ${SHUTDOWN_TIMEOUT_MS}ms`);
}

/** Opens the profile's database. `readOnly` is false only for the seeding step. */
async function openDatabase(profile, readOnly) {
  const path = join(profile, "data", "db", "jarvis.sqlite");
  if (!existsSync(path)) {
    throw new Error(`the profile has no database at ${path}`);
  }
  let DatabaseSync;
  try {
    ({ DatabaseSync } = await import("node:sqlite"));
  } catch (error) {
    throw new Error(`this journey needs Node's built-in node:sqlite module: ${error.message}`);
  }
  return new DatabaseSync(path, { readOnly });
}

/**
 * Runs `read` against the profile's database, opened **read-only**.
 *
 * Read-only so a check can never be the thing that changes what it observes; closed in a `finally` so a
 * failing check cannot leave the file locked for the next one.
 */
async function withDatabase(profile, read) {
  const database = await openDatabase(profile, true);
  try {
    return read(database) ?? null;
  } finally {
    database.close();
  }
}

/**
 * A canonical `ToolIdentity` document, spelled exactly as the domain's serializer emits it.
 *
 * This is the value the unique index deduplicates on, so it has to be *the type's own* wire form rather
 * than a shape that merely looks right — `capability` is the canonical `namespace.name@major` string,
 * `source` is a struct, and `schema_fingerprint` is the `sha256:<hex>` string the type's `Display`
 * produces. The domain refuses any other spelling, so a seeded row with a near-miss shape would be read
 * as corruption rather than as a reservation, and the journey would fail for the wrong reason.
 */
function toolIdentityDocument() {
  return JSON.stringify({
    // `namespace.name@major` with **exactly one dot** — the canonical form the type parses. A second
    // dot makes the name non-canonical, and a document the type refuses is read as corruption rather
    // than as a reservation, so a near-miss shape here would fail the journey for the wrong reason.
    capability: "acme.mail@1",
    source: { kind: "connector", owner: "acme.mail", version: "1.0.0" },
    schema_fingerprint: `sha256:${"ab".repeat(32)}`,
  });
}

/**
 * Seeds one stranded tool-call row, exactly as a previous daemon would have left it.
 *
 * `state` is what decides the recovery the row gets: `reserved` never dispatched, `executing` did.
 * `dispatched_at` is set for the dispatched case — the reader refuses a state that requires a dispatch
 * without one, which is the `TLS-006` invariant, so a fixture that omitted it would be refused as
 * corrupt rather than exercised.
 *
 * The row is written through the profile's real schema (foreign keys on, so the run it names must
 * exist). One run is created through the API first and its identifier is passed in.
 */
async function seedStrandedCall(profile, runId, { record, callId, key, state, dispatchedAt }) {
  const database = await openDatabase(profile, false);
  try {
    // The principal and workspace are read from the run the create path wrote, so the reservation's
    // scope columns are the ones the daemon itself would use rather than invented values.
    const scope = database
      .prepare("SELECT workspace_id, principal_id FROM agent_runs WHERE id = ?")
      .get(runId);
    if (!scope) {
      throw new Error(`the run ${runId} is not present to seed against`);
    }
    database
      .prepare(
        "INSERT INTO tool_call_records (id, call_id, workspace_id, principal_id, run_id, " +
          "tool_identity_json, idempotency_key, attempt, operation, state, version, dispatched_at, " +
          "outcome, no_effect_confirmed, created_at, updated_at) " +
          "VALUES (?, ?, ?, ?, ?, ?, ?, 1, 'execute', ?, 1, ?, NULL, 0, ?, ?)",
      )
      .run(
        record,
        callId,
        scope.workspace_id,
        scope.principal_id,
        runId,
        toolIdentityDocument(),
        key,
        state,
        dispatchedAt,
        "2026-09-27T12:00:00Z",
        "2026-09-27T12:00:00Z",
      );
  } finally {
    database.close();
  }
}

/** Reads one stranded row's state and outcome back. */
async function readCall(profile, record) {
  return withDatabase(profile, (database) =>
    database
      .prepare("SELECT state, outcome, dispatched_at FROM tool_call_records WHERE id = ?")
      .get(record),
  );
}

async function main() {
  const { daemon, client } = resolveBinaries();
  const profile = mkdtempSync(join(tmpdir(), "jarvis-e2e-toolrecovery-"));
  let running = null;

  try {
    // ---------------------------------------------------------------------
    // 1. Seed. The daemon runs once first so the profile has a migrated database and an enrolled
    //    client, and a run exists for the ledger row's foreign key.
    // ---------------------------------------------------------------------
    const first = await startAndWait(daemon, client, profile);
    running = first.child;
    pass("the daemon started and reported ready on a fresh profile");

    const credentialPath = join(profile, "config", "client-credential");
    if (!existsSync(credentialPath)) {
      throw new Error(`the profile has no client credential at ${credentialPath}`);
    }

    // A run is created so the ledger row's `run_id` foreign key resolves. Its own execution is
    // irrelevant here — the ledger is seeded directly, because no executor exists to make a call
    // through the API, and inventing one would be testing a path the product does not have.
    const suffix = process.platform === "win32" ? ".exe" : "";
    const clientBinary = join(join(process.argv[2]), `jarvis${suffix}`);
    const created = run(clientBinary, [
      "--profile",
      profile,
      "ask",
      "seed a run for the ledger journey",
    ]);
    if (created.status !== 0) {
      throw new Error(`seeding a run failed: ${created.stderr || created.stdout}`);
    }

    await stop(running);
    running = null;
    pass("the daemon stopped so the profile holds a clean, migrated database");

    const runId = await withDatabase(profile, (database) =>
      database.prepare("SELECT id FROM agent_runs ORDER BY created_at DESC LIMIT 1").get(),
    ).then((row) => {
      if (!row?.id) {
        throw new Error("no run was recorded to attach the ledger rows to");
      }
      return row.id;
    });

    const reserved = {
      record: "01930000-0000-7000-8000-000000000001",
      callId: "01930000-0000-7000-8000-0000000000a1",
      key: "stranded-pre-dispatch",
      state: "reserved",
      dispatchedAt: null,
    };
    const executing = {
      record: "01930000-0000-7000-8000-000000000002",
      callId: "01930000-0000-7000-8000-0000000000a2",
      key: "stranded-dispatched",
      state: "executing",
      dispatchedAt: "2026-09-27T12:00:30Z",
    };
    const finished = {
      record: "01930000-0000-7000-8000-000000000003",
      callId: "01930000-0000-7000-8000-0000000000a3",
      key: "already-terminal",
      state: "succeeded",
      dispatchedAt: "2026-09-27T12:00:30Z",
    };
    await seedStrandedCall(profile, runId, reserved);
    await seedStrandedCall(profile, runId, executing);
    await seedStrandedCall(profile, runId, finished);
    // The terminal row needs an outcome; a terminal row with none is refused as corruption by the
    // reader, which is the invariant that makes this fixture an honest one.
    {
      const database = await openDatabase(profile, false);
      try {
        database
          .prepare("UPDATE tool_call_records SET outcome = 'tool.provider_error' WHERE id = ?")
          .run(finished.record);
      } finally {
        database.close();
      }
    }
    pass("a pre-dispatch, a dispatched, and a terminal stranded call are seeded");

    // ---------------------------------------------------------------------
    // 2. Restart. Recovery must settle the two stranded rows and leave the terminal one alone.
    // ---------------------------------------------------------------------
    const second = await startAndWait(daemon, client, profile);
    running = second.child;
    pass("the daemon restarted and reported ready");

    const reservedAfter = await readCall(profile, reserved.record);
    if (reservedAfter?.state !== "cancelled") {
      fail(
        "**a stranded pre-dispatch reservation must be settled as cancelled**: its holder is gone, so " +
          "leaving it reserved makes every later retry of this key answer InFlight — waiting on a dead " +
          "process",
        JSON.stringify(reservedAfter),
      );
    } else if (reservedAfter.outcome !== "tool.cancelled") {
      fail(
        "a cancelled row must record the cancellation outcome, or the reader refuses it as corrupt",
        JSON.stringify(reservedAfter),
      );
    } else if (reservedAfter.dispatched_at !== null) {
      fail("nothing reached the provider, so no dispatch may be recorded", JSON.stringify(reservedAfter));
    } else {
      pass("the stranded pre-dispatch reservation is settled as cancelled with a recorded outcome");
    }

    const executingAfter = await readCall(profile, executing.record);
    if (executingAfter?.state !== "reconciling") {
      fail(
        "**a stranded dispatched call must be moved to reconciling**: its outcome is unknown, so a " +
          "retry must be told to reconcile rather than to wait",
        JSON.stringify(executingAfter),
      );
    } else if (executingAfter.dispatched_at === null) {
      fail("the dispatch fact must survive the recovery write", JSON.stringify(executingAfter));
    } else {
      pass("the stranded dispatched call is moved to reconciling and keeps its dispatch fact");
    }

    const finishedAfter = await readCall(profile, finished.record);
    if (finishedAfter?.state !== "succeeded" || finishedAfter.outcome !== "tool.provider_error") {
      fail(
        "a terminal call must not be rewritten: a recorded outcome is not re-derived",
        JSON.stringify(finishedAfter),
      );
    } else {
      pass("the terminal call is left exactly as it was");
    }

    // ---------------------------------------------------------------------
    // 3. The report is not dropped. The daemon logged the run pass's summary and, before this
    //    round, silently discarded the tool-call one.
    // ---------------------------------------------------------------------
    const log = logSince(profile, second.logOffset);
    if (!/recovered tool calls left incomplete by the previous shutdown/.test(log)) {
      fail(
        "**the daemon must report what the tool-call recovery pass settled**: a dropped report means " +
          "an operator cannot learn from a crash that work was stranded",
        log.slice(-4000),
      );
    } else if (!/"reconciling":1/.test(log) || !/"cancelled":1/.test(log)) {
      // The counts are asserted because the two settle differently: a reconciled call is an effect
      // whose existence is unknown, a cancelled one is a reservation whose holder died. A report
      // that named only a total would leave an operator unable to tell which they are looking at.
      // Matched in the `tracing` JSON field form (`"reconciling":1`), which is what the daemon emits.
      fail(
        "the report must name the counts, so a reader can tell a converted call from a cancelled one",
        log.slice(-4000),
      );
    } else {
      pass("the daemon logs the tool-call recovery report with its counts");
    }

    // ---------------------------------------------------------------------
    // 4. Idempotence. A second restart must change nothing, which is what makes this safe to run on
    //    every start rather than only after a crash.
    // ---------------------------------------------------------------------
    await stop(running);
    running = null;
    const third = await startAndWait(daemon, client, profile);
    running = third.child;
    const reservedAgain = await readCall(profile, reserved.record);
    const executingAgain = await readCall(profile, executing.record);
    if (reservedAgain?.state !== "cancelled" || executingAgain?.state !== "reconciling") {
      fail(
        "a second recovery pass must be a no-op: the rows are already settled",
        `${JSON.stringify(reservedAgain)} ${JSON.stringify(executingAgain)}`,
      );
    } else if (/recovered tool calls left incomplete by the previous shutdown/.test(logSince(profile, third.logOffset))) {
      // The rows are settled, so there is nothing to report — a second `warn` would mean the pass
      // re-settled them, which is the defect that makes recovery unsafe to run unconditionally.
      fail(
        "a settled profile must report no tool-call recovery on the next start",
        logSince(profile, third.logOffset).slice(-4000),
      );
    } else {
      pass("a second restart changes nothing and reports nothing to recover");
    }
  } finally {
    if (running) {
      await stop(running);
    }
    rmSync(profile, { recursive: true, force: true });
  }

  if (failures > 0) {
    console.error(`\n${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("\nall tool-call recovery journey checks passed");
}

await main();
